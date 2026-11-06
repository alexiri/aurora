#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod common;

use aurora_config::SignupMode;
use aurora_core::token::{self, TokenPrefix};
use common::{Harness, cookie};

const ORIGIN: &str = "http://localhost:4100";
use http::{HeaderMap, Method, StatusCode};
use serde_json::{Value, json};
use std::time::Duration;

const PW: &str = "correct horse battery staple";

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
    let ck = session.map(cookie);
    let mut hs = vec![("origin", ORIGIN)];
    if let Some(c) = ck.as_deref() {
        hs.push(("cookie", c));
    }
    h.send(Method::POST, uri, &hs, Some(body)).await
}

fn session_of(headers: &HeaderMap) -> String {
    let c = headers.get("set-cookie").unwrap().to_str().unwrap();
    c.split(';')
        .next()
        .unwrap()
        .strip_prefix("aurora_session=")
        .unwrap()
        .to_owned()
}

async fn signup(h: &Harness, email: &str) -> (StatusCode, HeaderMap, Value) {
    post(
        h,
        "/v1/auth/signup",
        json!({"email": email, "password": PW, "display_name": "Ann"}),
        None,
    )
    .await
}

async fn whoami(h: &Harness, sess: &str) -> StatusCode {
    let c = cookie(sess);
    h.send(Method::GET, "/t/whoami", &[("cookie", &c)], None)
        .await
        .0
}

async fn mail_link(h: &Harness, n: usize) -> String {
    for _ in 0..100 {
        let sent = h.mail.sent();
        if sent.len() >= n {
            return sent[n - 1].link.clone();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("mail {n} not sent");
}

fn tok_of(link: &str) -> String {
    link.split("token=").nth(1).unwrap().to_owned()
}

#[tokio::test]
async fn open_signup_creates_org_and_cookie() {
    let h = open().await;
    let (s, hd, v) = signup(&h, "  Ann@Example.com ").await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(v["user"]["email"], "ann@example.com");
    assert!(v["user"]["email_verified_at"].is_null());
    let c = hd.get("set-cookie").unwrap().to_str().unwrap();
    assert!(c.starts_with("aurora_session=aus_"));
    assert!(c.contains("HttpOnly") && c.contains("SameSite=Lax") && c.contains("Path=/"));
    assert!(!c.contains("Secure"), "http external url");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM organizations o JOIN org_members m ON m.org_id=o.id
         JOIN workspaces w ON w.org_id=o.id JOIN agents a ON a.workspace_id=w.id
         WHERE o.name = 'Ann''s org' AND m.role='owner'",
    )
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1);
    assert_eq!(whoami(&h, &session_of(&hd)).await, StatusCode::OK);
    // verification email
    assert!(mail_link(&h, 1).await.contains("/auth/verify?token=aui_"));
    // duplicate
    let (s, _, v) = signup(&h, "ANN@example.com").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("email_taken"))
    );
}

#[tokio::test]
async fn secure_cookie_on_https() {
    let h = Harness::with_config(|c| {
        c.signup_mode = SignupMode::Open;
        c.external_url = "https://aurora.example.com".into();
    })
    .await;
    let (_, hd, _) = signup(&h, "a@example.com").await;
    assert!(
        hd.get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("; Secure")
    );
}

#[tokio::test]
async fn weak_passwords_rejected() {
    let h = open().await;
    for pw in ["short", "UnBelievable", "x".repeat(257).as_str()] {
        let (s, _, v) = post(
            &h,
            "/v1/auth/signup",
            json!({"email":"a@example.com","password":pw,"display_name":"A"}),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{pw}");
        assert_eq!(v["error"]["details"]["fields"][0]["path"], "/password");
    }
}

#[tokio::test]
async fn invite_only_modes() {
    let h = Harness::with_config(|c| {
        c.signup_mode = SignupMode::InviteOnly;
        c.auth.instance_admins = vec!["boss@example.com".into()];
    })
    .await;
    let (s, _, v) = signup(&h, "x@example.com").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("signup_closed"))
    );

    // admin-listed: allowed, unverified => flag not granted
    let (s, _, v) = signup(&h, "boss@example.com").await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(v["user"]["is_instance_admin"], false);

    // invitation token
    let owner = h.user("owner@example.com", false).await;
    let org = h.org().await;
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
    let (s, _, _) = post(
        &h,
        "/v1/auth/signup",
        json!({"email":"other@example.com","password":PW,"display_name":"O","invitation_token":tok}),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "token for another email");
    let (s, _, v) = post(
        &h,
        "/v1/auth/signup",
        json!({"email":"inv@example.com","password":PW,"display_name":"I","invitation_token":tok}),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert!(!v["user"]["email_verified_at"].is_null());
    let st: String = sqlx::query_scalar("SELECT status FROM invitations WHERE org_id=$1")
        .bind(org)
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(st, "pending");
}

#[tokio::test]
async fn sso_jit_closed() {
    let h = Harness::with_config(|c| c.signup_mode = SignupMode::SsoJit).await;
    let (s, _, v) = signup(&h, "x@example.com").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("signup_closed"))
    );
}

#[tokio::test]
async fn login_logout_and_errors() {
    let h = open().await;
    signup(&h, "ann@example.com").await;
    let (s, _, v) = post(
        &h,
        "/v1/auth/login",
        json!({"email":"ann@example.com","password":"nope-nope-nope"}),
        None,
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("invalid_credentials"))
    );
    let (s, _, v) = post(
        &h,
        "/v1/auth/login",
        json!({"email":"ghost@example.com","password":PW}),
        None,
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("invalid_credentials"))
    );
    let (s, hd, _) = post(
        &h,
        "/v1/auth/login",
        json!({"email":"ANN@example.com","password":PW}),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let sess = session_of(&hd);
    assert_eq!(whoami(&h, &sess).await, StatusCode::OK);
    let last: bool = sqlx::query_scalar(
        "SELECT last_login_at IS NOT NULL FROM users WHERE email='ann@example.com'",
    )
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert!(last);

    let (s, hd, _) = post(&h, "/v1/auth/logout", json!({}), Some(&sess)).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert!(
        hd.get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    assert_eq!(whoami(&h, &sess).await, StatusCode::UNAUTHORIZED);

    sqlx::query("UPDATE users SET disabled_at=now()")
        .execute(h.pool())
        .await
        .unwrap();
    let (s, _, v) = post(
        &h,
        "/v1/auth/login",
        json!({"email":"ann@example.com","password":PW}),
        None,
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("account_disabled"))
    );
}

#[tokio::test]
async fn login_rate_limited() {
    let h = open().await;
    for _ in 0..5 {
        let (s, _, _) = post(
            &h,
            "/v1/auth/login",
            json!({"email":"a@example.com","password":"x"}),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
    let (s, hd, v) = post(
        &h,
        "/v1/auth/login",
        json!({"email":"a@example.com","password":"x"}),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(v["error"]["code"], "rate_limited");
    assert!(hd.get("retry-after").is_some());
}

#[tokio::test]
async fn verify_email_single_use_and_expiry() {
    let h = open().await;
    let (_, hd, _) = signup(&h, "ann@example.com").await;
    let sess = session_of(&hd);
    let tok = tok_of(&mail_link(&h, 1).await);
    let (s, _, v) = post(&h, "/v1/auth/verify-email", json!({"token": tok}), None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(!v["user"]["email_verified_at"].is_null());
    let (s, _, v) = post(&h, "/v1/auth/verify-email", json!({"token": tok}), None).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_token"))
    );
    let (s, _, v) = post(&h, "/v1/auth/verify-email/resend", json!({}), Some(&sess)).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("already_verified"))
    );

    // expiry
    sqlx::query("UPDATE users SET email_verified_at=NULL")
        .execute(h.pool())
        .await
        .unwrap();
    let (s, _, _) = post(&h, "/v1/auth/verify-email/resend", json!({}), Some(&sess)).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let tok2 = tok_of(&mail_link(&h, 2).await);
    sqlx::query("UPDATE email_tokens SET expires_at = now() - interval '1 second'")
        .execute(h.pool())
        .await
        .unwrap();
    let (s, _, _) = post(&h, "/v1/auth/verify-email", json!({"token": tok2}), None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn reset_revokes_all_sessions() {
    let h = open().await;
    let (_, hd, _) = signup(&h, "ann@example.com").await;
    let sess = session_of(&hd);
    let (s, _, _) = post(
        &h,
        "/v1/auth/password/forgot",
        json!({"email":"ghost@example.com"}),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let (s, _, _) = post(
        &h,
        "/v1/auth/password/forgot",
        json!({"email":"ann@example.com"}),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    // mail 1 is the verification email, 2 the reset
    let link = mail_link(&h, 2).await;
    assert!(link.contains("/auth/reset?token="));
    let tok = tok_of(&link);
    let (s, _, _) = post(
        &h,
        "/v1/auth/password/reset",
        json!({"token": tok, "password":"short"}),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let newpw = "another long passphrase 42";
    let (s, _, _) = post(
        &h,
        "/v1/auth/password/reset",
        json!({"token": tok, "password": newpw}),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(whoami(&h, &sess).await, StatusCode::UNAUTHORIZED);
    let (s, _, v) = post(
        &h,
        "/v1/auth/password/reset",
        json!({"token": tok, "password": newpw}),
        None,
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_token"))
    );
    let (s, _, _) = post(
        &h,
        "/v1/auth/login",
        json!({"email":"ann@example.com","password":newpw}),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn change_password_and_revoke_others() {
    let h = open().await;
    let (_, hd, _) = signup(&h, "ann@example.com").await;
    let sess = session_of(&hd);
    let (_, hd2, _) = post(
        &h,
        "/v1/auth/login",
        json!({"email":"ann@example.com","password":PW}),
        None,
    )
    .await;
    let other = session_of(&hd2);

    let (s, _, v) = post(
        &h,
        "/v1/auth/password/change",
        json!({"current_password":"bad bad bad bad","new_password":"yet another long one 1"}),
        Some(&sess),
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("invalid_credentials"))
    );
    let (s, _, _) = post(
        &h,
        "/v1/auth/password/change",
        json!({"current_password":PW,"new_password":"short"}),
        Some(&sess),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _, _) = post(
        &h,
        "/v1/auth/password/change",
        json!({"current_password":PW,"new_password":"yet another long one 1"}),
        Some(&sess),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    let (s, _, v) = post(
        &h,
        "/v1/auth/sessions/revoke-others",
        json!({}),
        Some(&sess),
    )
    .await;
    assert_eq!((s, v["revoked"].as_u64()), (StatusCode::OK, Some(1)));
    assert_eq!(whoami(&h, &sess).await, StatusCode::OK);
    assert_eq!(whoami(&h, &other).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_keys_refused_on_session_only_routes() {
    let h = open().await;
    let (_, hd, v) = signup(&h, "ann@example.com").await;
    let _ = hd;
    let uid: uuid::Uuid = v["user"]["id"].as_str().unwrap().parse().unwrap();
    let org: uuid::Uuid = sqlx::query_scalar("SELECT org_id FROM org_members WHERE user_id=$1")
        .bind(uid)
        .fetch_one(h.pool())
        .await
        .unwrap();
    let (_, key) = h.api_key(uid, org, None, false).await;
    let b = common::bearer(&key);
    for (uri, body) in [
        (
            "/v1/auth/password/change",
            json!({"current_password":PW,"new_password":"yet another long one 1"}),
        ),
        ("/v1/auth/sessions/revoke-others", json!({})),
    ] {
        let (s, _, v) = h
            .send(Method::POST, uri, &[("authorization", &b)], Some(body))
            .await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("session_required"))
        );
    }
}

#[tokio::test]
async fn instance_admin_flag_only_when_verified() {
    let h = open().await;
    let (_, _, v) = signup(&h, "boss@example.com").await;
    assert_eq!(v["user"]["is_instance_admin"], false);
    let (_, _, v) = post(
        &h,
        "/v1/auth/login",
        json!({"email":"boss@example.com","password":PW}),
        None,
    )
    .await;
    assert_eq!(v["user"]["is_instance_admin"], false, "unverified");
    sqlx::query("UPDATE users SET email_verified_at=now()")
        .execute(h.pool())
        .await
        .unwrap();
    let (_, _, v) = post(
        &h,
        "/v1/auth/login",
        json!({"email":"boss@example.com","password":PW}),
        None,
    )
    .await;
    assert_eq!(v["user"]["is_instance_admin"], true);
}

#[tokio::test]
async fn signup_rate_limited() {
    let h = open().await;
    for i in 0..5 {
        let (s, _, _) = signup(&h, &format!("u{i}@example.com")).await;
        assert_eq!(s, StatusCode::CREATED);
    }
    let (s, hd, _) = signup(&h, "u9@example.com").await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert!(hd.get("retry-after").is_some());
}
