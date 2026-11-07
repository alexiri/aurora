#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! OIDC sign-in against the in-process mock IdP (BE-015).

use crate::common::{Harness, ORIGIN};
use aurora_config::{OidcJit, OidcProvider, Redacted, SignupMode};
use aurora_testkit::oidc::{CLIENT_ID, CLIENT_SECRET, MockIdp};
use http::{Method, StatusCode};
use uuid::Uuid;

const PID: &str = "mock";

fn provider(idp: &MockIdp, f: impl FnOnce(&mut OidcProvider)) -> OidcProvider {
    let mut p = OidcProvider {
        id: PID.into(),
        display_name: Some("Mock SSO".into()),
        issuer: idp.issuer().into(),
        client_id: CLIENT_ID.into(),
        client_secret_env: "MOCK_SECRET".into(),
        ..OidcProvider::default()
    };
    f(&mut p);
    p
}

async fn harness(
    idp: &MockIdp,
    mode: SignupMode,
    tweak: impl FnOnce(&mut OidcProvider),
    admins: &[&str],
) -> Harness {
    let p = provider(idp, tweak);
    let admins: Vec<String> = admins.iter().map(|s| (*s).to_owned()).collect();
    Harness::with_config(move |c| {
        c.signup_mode = mode;
        c.auth.oidc_providers = vec![p];
        c.auth.instance_admins = admins;
        c.oidc_client_secrets
            .insert(PID.into(), Redacted::new(CLIENT_SECRET.into()));
    })
    .await
}

fn header(h: &http::HeaderMap, name: &str) -> Vec<String> {
    h.get_all(name)
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect()
}

fn flow_cookie(h: &http::HeaderMap) -> String {
    let c = header(h, "set-cookie")
        .into_iter()
        .find(|c| c.starts_with("__Host-aurora_oidc="))
        .expect("oidc cookie");
    c.split(';').next().unwrap().to_owned()
}

/// Run `start` and the provider's authorize step; returns `(callback path+query, cookie)`.
async fn authorize(h: &Harness, return_to: Option<&str>) -> (String, String) {
    let uri = match return_to {
        Some(r) => format!(
            "/v1/auth/oidc/{PID}/start?return_to={}",
            percent_encoding::utf8_percent_encode(r, percent_encoding::NON_ALPHANUMERIC)
        ),
        None => format!("/v1/auth/oidc/{PID}/start"),
    };
    let (st, hd, _) = h.send(Method::GET, &uri, &[], None).await;
    assert_eq!(st, StatusCode::FOUND);
    let cookie = flow_cookie(&hd);
    let cookies = header(&hd, "set-cookie").join(";");
    assert!(
        cookies.contains("HttpOnly") && cookies.contains("Secure") && cookies.contains("Path=/")
    );
    let loc = header(&hd, "location").remove(0);
    assert!(loc.contains("code_challenge_method=S256") && loc.contains("nonce="));
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let res = http.get(&loc).send().await.unwrap();
    assert_eq!(res.status().as_u16(), 303);
    let back = res.headers()["location"].to_str().unwrap().to_owned();
    let path = back
        .strip_prefix(ORIGIN)
        .expect("redirect_uri on external url");
    (path.to_owned(), cookie)
}

async fn callback(h: &Harness, path: &str, cookie: &str) -> (StatusCode, http::HeaderMap) {
    let (st, hd, _) = h.send(Method::GET, path, &[("cookie", cookie)], None).await;
    (st, hd)
}

fn location(h: &http::HeaderMap) -> String {
    header(h, "location").remove(0)
}

fn has_session(h: &http::HeaderMap) -> bool {
    header(h, "set-cookie")
        .iter()
        .any(|c| c.starts_with("aurora_session=aus_"))
}

async fn count(h: &Harness, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(h.pool()).await.unwrap()
}

#[tokio::test]
async fn happy_path_new_user_open_mode() {
    let idp = MockIdp::start().await;
    idp.identity("s1", "New.User@Example.com", true);
    let h = harness(&idp, SignupMode::Open, |_| {}, &[]).await;
    let (path, cookie) = authorize(&h, Some("/orgs/x?y=1")).await;
    let (st, hd) = callback(&h, &path, &cookie).await;
    assert_eq!(st, StatusCode::FOUND);
    assert_eq!(location(&hd), "/orgs/x?y=1");
    assert!(has_session(&hd));
    assert!(
        header(&hd, "set-cookie")
            .iter()
            .any(|c| c.starts_with("__Host-aurora_oidc=;"))
    );
    assert_eq!(count(&h, "SELECT count(*) FROM users WHERE email = 'new.user@example.com' AND email_verified_at IS NOT NULL AND password_hash IS NULL").await, 1);
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM user_identities WHERE provider_id = 'mock' AND subject = 's1'"
        )
        .await,
        1
    );
    assert_eq!(count(&h, "SELECT count(*) FROM organizations").await, 1);
    assert_eq!(count(&h, "SELECT count(*) FROM workspaces").await, 1);
}

#[tokio::test]
async fn default_return_to_and_session_works() {
    let idp = MockIdp::start().await;
    let h = harness(&idp, SignupMode::Open, |_| {}, &[]).await;
    let (path, cookie) = authorize(&h, None).await;
    let (st, hd) = callback(&h, &path, &cookie).await;
    assert_eq!(st, StatusCode::FOUND);
    assert_eq!(location(&hd), "/");
    let sess = header(&hd, "set-cookie")
        .into_iter()
        .find(|c| c.starts_with("aurora_session="))
        .unwrap();
    let sess = sess.split(';').next().unwrap().to_owned();
    let (st, _) = h.get("/v1/me", &[("cookie", &sess)]).await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn existing_identity_signs_in_without_new_user() {
    let idp = MockIdp::start().await;
    idp.identity("s1", "a@example.com", true);
    let h = harness(&idp, SignupMode::Open, |_| {}, &[]).await;
    for _ in 0..2 {
        let (path, cookie) = authorize(&h, None).await;
        let (_, hd) = callback(&h, &path, &cookie).await;
        assert!(has_session(&hd));
    }
    assert_eq!(count(&h, "SELECT count(*) FROM users").await, 1);
    assert_eq!(count(&h, "SELECT count(*) FROM user_identities").await, 1);
    // Email change at the provider does not matter once linked.
    idp.identity("s1", "other@example.com", true);
    let (path, cookie) = authorize(&h, None).await;
    let (_, hd) = callback(&h, &path, &cookie).await;
    assert!(has_session(&hd));
    assert_eq!(count(&h, "SELECT count(*) FROM users").await, 1);
}

#[tokio::test]
async fn links_existing_user_by_verified_email() {
    let idp = MockIdp::start().await;
    idp.identity("s9", "linked@example.com", true);
    let h = harness(&idp, SignupMode::InviteOnly, |_| {}, &[]).await;
    let uid = h.user("linked@example.com", false).await;
    let (path, cookie) = authorize(&h, None).await;
    let (_, hd) = callback(&h, &path, &cookie).await;
    assert!(has_session(&hd));
    let linked: Uuid =
        sqlx::query_scalar("SELECT user_id FROM user_identities WHERE subject = 's9'")
            .fetch_one(h.pool())
            .await
            .unwrap();
    assert_eq!(linked, uid);
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM users WHERE email_verified_at IS NOT NULL"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn unverified_email_refused_when_required() {
    let idp = MockIdp::start().await;
    idp.identity("s2", "u@example.com", false);
    let h = harness(&idp, SignupMode::Open, |_| {}, &[]).await;
    let (path, cookie) = authorize(&h, None).await;
    let (st, hd) = callback(&h, &path, &cookie).await;
    assert_eq!(st, StatusCode::FOUND);
    assert_eq!(location(&hd), "/auth/login?error=email_unverified");
    assert!(!has_session(&hd));
    assert_eq!(count(&h, "SELECT count(*) FROM users").await, 0);
}

#[tokio::test]
async fn unverified_email_allowed_but_never_links() {
    let idp = MockIdp::start().await;
    idp.identity("s3", "victim@example.com", false);
    let h = harness(
        &idp,
        SignupMode::Open,
        |p| p.require_email_verified = false,
        &[],
    )
    .await;
    h.user("victim@example.com", false).await;
    let (path, cookie) = authorize(&h, None).await;
    let (_, hd) = callback(&h, &path, &cookie).await;
    assert_eq!(location(&hd), "/auth/login?error=email_unverified");
    assert_eq!(count(&h, "SELECT count(*) FROM user_identities").await, 0);

    idp.identity("s4", "fresh@example.com", false);
    let (path, cookie) = authorize(&h, None).await;
    let (_, hd) = callback(&h, &path, &cookie).await;
    assert!(has_session(&hd));
    assert_eq!(count(&h, "SELECT count(*) FROM users WHERE email = 'fresh@example.com' AND email_verified_at IS NULL").await, 1);
}

#[tokio::test]
async fn domain_allow_list() {
    let idp = MockIdp::start().await;
    let h = harness(
        &idp,
        SignupMode::Open,
        |p| p.allowed_email_domains = vec!["corp.example".into()],
        &[],
    )
    .await;
    idp.identity("d1", "x@other.example", true);
    let (path, cookie) = authorize(&h, None).await;
    let (_, hd) = callback(&h, &path, &cookie).await;
    assert_eq!(location(&hd), "/auth/login?error=email_domain_not_allowed");
    idp.identity("d2", "x@Corp.Example", true);
    let (path, cookie) = authorize(&h, None).await;
    let (_, hd) = callback(&h, &path, &cookie).await;
    assert!(has_session(&hd));
}

#[tokio::test]
async fn state_cookie_mismatch_and_missing() {
    let idp = MockIdp::start().await;
    let h = harness(&idp, SignupMode::Open, |_| {}, &[]).await;
    let (path, _c1) = authorize(&h, None).await;
    let (_, c2) = authorize(&h, None).await;
    let (st, hd) = callback(&h, &path, &c2).await;
    assert_eq!(st, StatusCode::FOUND);
    assert_eq!(location(&hd), "/auth/login?error=oidc_state_invalid");
    assert!(!has_session(&hd));
    // No cookie at all.
    let (st, hd, _) = h.send(Method::GET, &path, &[], None).await;
    assert_eq!(st, StatusCode::FOUND);
    assert_eq!(location(&hd), "/auth/login?error=oidc_state_invalid");
    // Tampered state with a matching cookie.
    let tampered = path.replacen("state=", "state=A", 1);
    let state_val = tampered
        .split("state=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap();
    let (_, hd) = callback(&h, &tampered, &format!("__Host-aurora_oidc={state_val}")).await;
    assert_eq!(location(&hd), "/auth/login?error=oidc_state_invalid");
    assert_eq!(count(&h, "SELECT count(*) FROM users").await, 0);
}

#[tokio::test]
async fn expired_state() {
    let idp = MockIdp::start().await;
    let h = harness(&idp, SignupMode::Open, |_| {}, &[]).await;
    let (st, hd, _) = h
        .send(
            Method::GET,
            &format!("/v1/auth/oidc/{PID}/start"),
            &[],
            None,
        )
        .await;
    assert_eq!(st, StatusCode::FOUND);
    let cookie = flow_cookie(&hd);
    // Forge an already-expired state with the real derived key.
    let flow = serde_json::json!({"p": PID, "n": "n", "v": "v", "r": "/", "e": 1});
    let sealed = h
        .state
        .derived
        .seal(
            aurora_crypto::Purpose::OidcState,
            format!("oidc:{PID}").as_bytes(),
            flow.to_string().as_bytes(),
        )
        .unwrap();
    assert_ne!(cookie, format!("__Host-aurora_oidc={sealed}"));
    let (_, hd) = callback(
        &h,
        &format!("/v1/auth/oidc/{PID}/callback?code=x&state={sealed}"),
        &format!("__Host-aurora_oidc={sealed}"),
    )
    .await;
    assert_eq!(location(&hd), "/auth/login?error=oidc_state_invalid");
}

#[tokio::test]
async fn wrong_nonce_rejected() {
    let idp = MockIdp::start().await;
    idp.set(|c| c.nonce = Some("not-the-nonce".into()));
    let h = harness(&idp, SignupMode::Open, |_| {}, &[]).await;
    let (path, cookie) = authorize(&h, None).await;
    let (_, hd) = callback(&h, &path, &cookie).await;
    assert_eq!(location(&hd), "/auth/login?error=oidc_failed");
    assert!(!has_session(&hd));
    assert_eq!(count(&h, "SELECT count(*) FROM users").await, 0);
}

#[tokio::test]
async fn expired_id_token_rejected() {
    let idp = MockIdp::start().await;
    idp.set(|c| c.exp_in = -3600);
    let h = harness(&idp, SignupMode::Open, |_| {}, &[]).await;
    let (path, cookie) = authorize(&h, None).await;
    let (_, hd) = callback(&h, &path, &cookie).await;
    assert_eq!(location(&hd), "/auth/login?error=oidc_failed");
}

#[tokio::test]
async fn bad_return_to_and_unknown_provider() {
    let idp = MockIdp::start().await;
    let h = harness(&idp, SignupMode::Open, |_| {}, &[]).await;
    for bad in [
        "https://evil.example/",
        "//evil.example",
        "/\\evil.example",
        "relative",
    ] {
        let uri = format!(
            "/v1/auth/oidc/{PID}/start?return_to={}",
            percent_encoding::utf8_percent_encode(bad, percent_encoding::NON_ALPHANUMERIC)
        );
        let (st, _, body) = h.send(Method::GET, &uri, &[], None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}");
        assert_eq!(body["error"]["details"]["fields"][0]["path"], "/return_to");
    }
    let (st, _, _) = h
        .send(Method::GET, "/v1/auth/oidc/nope/start", &[], None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, _) = h
        .send(
            Method::GET,
            "/v1/auth/oidc/nope/callback?code=x&state=y",
            &[],
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

async fn jit_seed(h: &Harness) -> (Uuid, Uuid, Uuid) {
    let org = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO organizations (id, slug, name, external_id) VALUES ($1,'acme','Acme','acme')",
    )
    .bind(org)
    .execute(h.pool())
    .await
    .unwrap();
    let a = Uuid::now_v7();
    let b = Uuid::now_v7();
    for (id, slug) in [(a, "eng"), (b, "ops")] {
        sqlx::query("INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1,$2,$3,$3)")
            .bind(id)
            .bind(org)
            .bind(slug)
            .execute(h.pool())
            .await
            .unwrap();
    }
    (org, a, b)
}

fn jit(p: &mut OidcProvider) {
    p.jit = Some(OidcJit {
        org_slug: "acme".into(),
        role: "member".into(),
        workspace_slugs: vec!["eng".into(), "missing".into()],
        workspace_role: String::new(),
    });
}

fn jit_approver(p: &mut OidcProvider) {
    p.jit = Some(OidcJit {
        org_slug: "acme".into(),
        role: "member".into(),
        workspace_slugs: vec!["eng".into()],
        workspace_role: "approver".into(),
    });
}

#[tokio::test]
async fn sso_jit_binds_as_approver_when_configured() {
    // RFE-012: `jit.workspace_role = approver` binds the JIT user as approver (never owner).
    let idp = MockIdp::start().await;
    idp.identity("j2", "jita@example.com", true);
    let h = harness(&idp, SignupMode::SsoJit, jit_approver, &[]).await;
    let (_org, eng, _ops) = jit_seed(&h).await;
    let (path, cookie) = authorize(&h, None).await;
    let (_, hd) = callback(&h, &path, &cookie).await;
    assert!(has_session(&hd));
    assert_eq!(
        count(
            &h,
            &format!("SELECT count(*) FROM workspace_access WHERE workspace_id = '{eng}' AND role = 'approver'")
        )
        .await,
        1
    );
}

#[tokio::test]
async fn sso_jit_adds_membership_and_bindings() {
    let idp = MockIdp::start().await;
    idp.identity("j1", "jit@example.com", true);
    let h = harness(&idp, SignupMode::SsoJit, jit, &[]).await;
    let (org, eng, ops) = jit_seed(&h).await;
    let (path, cookie) = authorize(&h, None).await;
    let (_, hd) = callback(&h, &path, &cookie).await;
    assert!(has_session(&hd));
    let role: String = sqlx::query_scalar(
        "SELECT m.role FROM org_members m JOIN users u ON u.id = m.user_id WHERE m.org_id = $1 AND u.email = 'jit@example.com'",
    )
    .bind(org)
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert_eq!(role, "member");
    assert_eq!(
        count(
            &h,
            &format!("SELECT count(*) FROM workspace_access WHERE workspace_id = '{eng}'")
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &h,
            &format!("SELECT count(*) FROM workspace_access WHERE workspace_id = '{ops}'")
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM users WHERE default_org_id IS NOT NULL"
        )
        .await,
        1
    );
    // Second sign-in is idempotent.
    let (path, cookie) = authorize(&h, None).await;
    let (_, hd) = callback(&h, &path, &cookie).await;
    assert!(has_session(&hd));
    assert_eq!(count(&h, "SELECT count(*) FROM org_members").await, 1);
}

#[tokio::test]
async fn sso_jit_without_rule_is_signup_closed() {
    let idp = MockIdp::start().await;
    idp.identity("j2", "nojit@example.com", true);
    let h = harness(&idp, SignupMode::SsoJit, |_| {}, &[]).await;
    let (path, cookie) = authorize(&h, None).await;
    let (_, hd) = callback(&h, &path, &cookie).await;
    assert_eq!(location(&hd), "/auth/login?error=signup_closed");
    assert!(!has_session(&hd));
    assert_eq!(count(&h, "SELECT count(*) FROM users").await, 0);
}

#[tokio::test]
async fn invite_only_closed_unless_listed_admin() {
    let idp = MockIdp::start().await;
    idp.identity("i1", "stranger@example.com", true);
    let h = harness(&idp, SignupMode::InviteOnly, |_| {}, &["boss@example.com"]).await;
    let (path, cookie) = authorize(&h, None).await;
    let (_, hd) = callback(&h, &path, &cookie).await;
    assert_eq!(location(&hd), "/auth/login?error=signup_closed");
    idp.identity("i2", "Boss@example.com", true);
    let (path, cookie) = authorize(&h, None).await;
    let (_, hd) = callback(&h, &path, &cookie).await;
    assert!(has_session(&hd));
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM users WHERE email = 'boss@example.com' AND is_instance_admin"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn listed_verified_email_gets_instance_admin_on_signin() {
    let idp = MockIdp::start().await;
    idp.identity("a1", "root@example.com", true);
    let h = harness(&idp, SignupMode::Open, |_| {}, &["root@example.com"]).await;
    let (path, cookie) = authorize(&h, None).await;
    let (_, hd) = callback(&h, &path, &cookie).await;
    assert!(has_session(&hd));
    assert_eq!(
        count(&h, "SELECT count(*) FROM users WHERE is_instance_admin").await,
        1
    );
}

#[tokio::test]
async fn instance_lists_providers() {
    let idp = MockIdp::start().await;
    let h = harness(&idp, SignupMode::Open, |_| {}, &[]).await;
    let (st, v) = h.get("/v1/instance", &[]).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["oidc_providers"][0]["id"], PID);
    assert_eq!(v["oidc_providers"][0]["display_name"], "Mock SSO");
}
