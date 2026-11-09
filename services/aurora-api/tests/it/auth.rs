#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Sessions and OIDC-only sign-in (phase 1, RFE-004, D-46; docs/04 §Authentication).

use crate::common::Harness;
use aurora_config::SignupMode;
use aurora_core::token::{self, TokenPrefix};
use http::{HeaderMap, Method, StatusCode};
use serde_json::{Value, json};

const ORIGIN: &str = "http://localhost:4100";

async fn open() -> Harness {
    Harness::with_config(|c| {
        c.signup_mode = SignupMode::Open;
        c.auth.instance_admins = vec!["Boss@Example.com".into()];
    })
    .await
}

async fn post(
    h: &Harness,
    uri: &str,
    body: Value,
    session: Option<&str>,
) -> (StatusCode, HeaderMap, Value) {
    let ck = session.map(|s| s.to_owned());
    let mut hs = vec![("origin", ORIGIN)];
    if let Some(c) = ck.as_deref() {
        hs.push(("cookie", c));
    }
    h.send(Method::POST, uri, &hs, Some(body)).await
}

async fn whoami(h: &Harness, cookie: &str) -> StatusCode {
    h.send(Method::GET, "/t/whoami", &[("cookie", cookie)], None)
        .await
        .0
}

async fn count(h: &Harness, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(h.pool()).await.unwrap()
}

#[tokio::test]
async fn open_sign_in_creates_user_org_and_cookie() {
    let h = open().await;
    let s = h.sign_in("  Ann@Example.com ").await;
    let (st, me) = h.get("/v1/me", &[("cookie", &s.cookie)]).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(me["user"]["email"], "ann@example.com");
    assert!(
        !me["user"]["email_verified_at"].is_null(),
        "provider-verified"
    );
    assert_eq!(me["user"]["is_instance_admin"], false);
    assert_eq!(me["memberships"][0]["role"], "owner");
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM organizations o JOIN org_members m ON m.org_id=o.id
             JOIN workspaces w ON w.org_id=o.id JOIN agents a ON a.workspace_id=w.id
             WHERE m.role='owner'"
        )
        .await,
        1
    );
    assert_eq!(whoami(&h, &s.cookie).await, StatusCode::OK);
    // No local credential can be stored: CONTRACT-001 dropped the local password column, and the
    // aurora-db `schema_matches_spec` test proves the column is absent.
    // A second sign-in of the same identity reuses the user.
    let again = h.sign_in("ann@example.com").await;
    assert_eq!(again.user_id, s.user_id);
    assert_eq!(count(&h, "SELECT count(*) FROM users").await, 1);
}

/// SEC-22: under an https external URL the session cookie is `Secure` as well as
/// `HttpOnly; SameSite=Lax`.
#[tokio::test]
async fn sec22_session_cookie_flags() {
    let h = open().await;
    let (path, flow) = h.oidc_authorize("dev", "", Some("a@example.com")).await;
    let (_, hd, _) = h.send(Method::GET, &path, &[("cookie", &flow)], None).await;
    let c = hd
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .find(|c| c.starts_with("aurora_session=aus_"))
        .unwrap();
    assert!(c.contains("HttpOnly") && c.contains("SameSite=Lax") && c.contains("Path=/"));
    assert!(!c.contains("Secure"), "http external url");

    let h = Harness::with_config(|c| {
        c.signup_mode = SignupMode::Open;
        c.external_url = "https://aurora.example.com".into();
    })
    .await;
    let (path, flow) = h.oidc_authorize("dev", "", Some("a@example.com")).await;
    let (_, hd, _) = h.send(Method::GET, &path, &[("cookie", &flow)], None).await;
    let c = hd
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .find(|c| c.starts_with("aurora_session=aus_"))
        .unwrap();
    assert!(
        c.contains("HttpOnly") && c.contains("SameSite=Lax") && c.contains("; Secure"),
        "{c}"
    );
}

/// RFE-004: the password, e-mail-verification and password-reset surfaces answer 404.
#[tokio::test]
async fn password_routes_are_gone() {
    let h = open().await;
    let s = h.sign_in("ann@example.com").await;
    for (m, uri) in [
        (Method::POST, "/v1/auth/signup"),
        (Method::POST, "/v1/auth/login"),
        (Method::POST, "/v1/auth/verify-email"),
        (Method::POST, "/v1/auth/verify-email/resend"),
        (Method::POST, "/v1/auth/password/forgot"),
        (Method::POST, "/v1/auth/password/reset"),
        (Method::POST, "/v1/auth/password/change"),
    ] {
        let (st, _, v) = h
            .send(
                m.clone(),
                uri,
                &[("origin", ORIGIN), ("cookie", &s.cookie)],
                Some(json!({"email":"a@example.com","password":"correct horse battery staple","token":"aui_x","display_name":"A"})),
            )
            .await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{m} {uri}: {v}");
    }
    // Neither is anything stored for them any more.
    assert_eq!(h.mail.sent().len(), 0, "no verification e-mail");
}

#[tokio::test]
async fn invite_only_sign_up_rules() {
    let h = Harness::with_config(|c| {
        c.signup_mode = SignupMode::InviteOnly;
        c.auth.instance_admins = vec!["boss@example.com".into()];
    })
    .await;
    // No invitation, not listed: refused and no user row is created.
    let e = h.try_sign_in("dev", "x@example.com").await.err().unwrap();
    assert_eq!(e, "signup_closed");
    assert_eq!(count(&h, "SELECT count(*) FROM users").await, 0);

    // Listed admin: allowed, flag granted (the provider verified the e-mail).
    let boss = h.sign_in("boss@example.com").await;
    let (_, me) = h.get("/v1/me", &[("cookie", &boss.cookie)]).await;
    assert_eq!(me["user"]["is_instance_admin"], true);
    assert!(
        me["memberships"].as_array().unwrap().is_empty(),
        "no org in invite_only"
    );

    // A pending invitation for that verified e-mail: the invitation is accepted by the sign-in.
    let owner = h.user("owner@example.com", false).await;
    let org = h.org().await;
    h.member(org, owner, "owner", "active").await;
    let tok = token::generate(TokenPrefix::Invitation).expose().clone();
    sqlx::query(
        "INSERT INTO invitations (id, org_id, email, role, token_hash, status, invited_by)
         VALUES (gen_random_uuid(), $1, 'inv@example.com', 'member', $2, 'pending', $3)",
    )
    .bind(org)
    .bind(&token::hash(&tok)[..])
    .bind(owner)
    .execute(h.pool())
    .await
    .unwrap();
    let other = h
        .try_sign_in("dev", "other@example.com")
        .await
        .err()
        .unwrap();
    assert_eq!(
        other, "signup_closed",
        "the invitation is for another e-mail"
    );
    let inv = h.sign_in("INV@example.com").await;
    let (_, me) = h.get("/v1/me", &[("cookie", &inv.cookie)]).await;
    assert_eq!(me["memberships"][0]["org"]["id"], org.to_string());
    assert_eq!(me["memberships"][0]["role"], "member");
    let st: String = sqlx::query_scalar("SELECT status FROM invitations WHERE org_id=$1")
        .bind(org)
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(st, "accepted");
}

#[tokio::test]
async fn sso_jit_closed_without_jit_or_invitation() {
    let h = Harness::with_config(|c| c.signup_mode = SignupMode::SsoJit).await;
    let e = h.try_sign_in("dev", "x@example.com").await.err().unwrap();
    assert_eq!(e, "signup_closed");
    assert_eq!(count(&h, "SELECT count(*) FROM users").await, 0);
}

#[tokio::test]
async fn logout_and_disabled_user() {
    let h = open().await;
    let s = h.sign_in("ann@example.com").await;
    assert!(
        count(
            &h,
            "SELECT count(*) FROM users WHERE email='ann@example.com' AND last_login_at IS NOT NULL"
        )
        .await
            == 1
    );
    let (st, hd, _) = post(&h, "/v1/auth/logout", json!({}), Some(&s.cookie)).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(
        hd.get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    assert_eq!(whoami(&h, &s.cookie).await, StatusCode::UNAUTHORIZED);

    sqlx::query("UPDATE users SET disabled_at=now()")
        .execute(h.pool())
        .await
        .unwrap();
    let e = h.try_sign_in("dev", "ann@example.com").await.err().unwrap();
    assert_eq!(e, "user_disabled");
}

#[tokio::test]
async fn revoke_others() {
    let h = open().await;
    let a = h.sign_in("ann@example.com").await;
    let b = h.sign_in("ann@example.com").await;
    assert_ne!(a.cookie, b.cookie);
    let (s, _, v) = post(
        &h,
        "/v1/auth/sessions/revoke-others",
        json!({}),
        Some(&a.cookie),
    )
    .await;
    assert_eq!((s, v["revoked"].as_u64()), (StatusCode::OK, Some(1)));
    assert_eq!(whoami(&h, &a.cookie).await, StatusCode::OK);
    assert_eq!(whoami(&h, &b.cookie).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_keys_refused_on_session_only_routes() {
    let h = open().await;
    let s = h.sign_in("ann@example.com").await;
    let org: uuid::Uuid = sqlx::query_scalar("SELECT org_id FROM org_members WHERE user_id=$1")
        .bind(s.user_id)
        .fetch_one(h.pool())
        .await
        .unwrap();
    let (_, key) = h.api_key(s.user_id, org, None, false).await;
    let b = crate::common::bearer(&key);
    let (st, _, v) = h
        .send(
            Method::POST,
            "/v1/auth/sessions/revoke-others",
            &[("authorization", &b)],
            Some(json!({})),
        )
        .await;
    assert_eq!(
        (st, v["error"]["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("session_required"))
    );
    // A key can never become a session: the OIDC callback is the only way in (SEC-42).
    let (st, hd, _) = h
        .send(
            Method::GET,
            "/v1/auth/oidc/dev/callback?code=x&state=y",
            &[("authorization", &b)],
            None,
        )
        .await;
    assert_eq!(st, StatusCode::FOUND);
    assert!(
        !hd.get_all("set-cookie")
            .iter()
            .any(|c| c.to_str().unwrap().starts_with("aurora_session="))
    );
}

/// SEC-42: the instance-admin flag needs an IdP identity with a *verified* e-mail; an unverified
/// ID token is refused outright, so it can neither sign in nor be promoted.
#[tokio::test]
async fn instance_admin_flag_needs_verified_idp_email() {
    let h = open().await;
    h.idp.identity("boss-sub", "boss@example.com", false);
    let e = h.try_sign_in_claims("dev").await.err().unwrap();
    assert_eq!(e, "email_unverified");
    assert_eq!(count(&h, "SELECT count(*) FROM users").await, 0);
    h.idp.identity("boss-sub", "boss@example.com", true);
    let s = h.try_sign_in_claims("dev").await.unwrap();
    let (_, me) = h.get("/v1/me", &[("cookie", &s.cookie)]).await;
    assert_eq!(me["user"]["is_instance_admin"], true);
}

#[tokio::test]
async fn oidc_start_and_callback_are_rate_limited() {
    let h = open().await;
    for _ in 0..20 {
        let (s, _, _) = h
            .send(Method::GET, "/v1/auth/oidc/dev/start", &[], None)
            .await;
        assert_eq!(s, StatusCode::FOUND);
    }
    let (s, hd, _) = h
        .send(Method::GET, "/v1/auth/oidc/dev/start", &[], None)
        .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert!(hd.get("retry-after").is_some());
    // The callback has its own bucket.
    for _ in 0..20 {
        let (s, _, _) = h
            .send(Method::GET, "/v1/auth/oidc/dev/callback", &[], None)
            .await;
        assert_eq!(s, StatusCode::FOUND);
    }
    let (s, _, _) = h
        .send(Method::GET, "/v1/auth/oidc/dev/callback", &[], None)
        .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
}

/// The development provider is the one in `aurora_testkit::oidc` (constants stay in step).
#[test]
fn dev_idp_client_constants_match_the_mock() {
    assert_eq!(
        aurora_testkit::oidc::CLIENT_ID,
        aurora_config::DEV_IDP_CLIENT_ID
    );
    assert_eq!(
        aurora_testkit::oidc::CLIENT_SECRET,
        aurora_config::DEV_IDP_CLIENT_SECRET
    );
}
