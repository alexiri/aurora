#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use http::Method;
use serde_json::{Value, json};

async fn call(h: &Harness, tok: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
    let c = cookie(tok);
    let (s, _, v) = h
        .send(m, uri, &[("cookie", &c), ("origin", ORIGIN)], body)
        .await;
    (s.as_u16(), v)
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

#[tokio::test]
async fn list_requires_instance_admin_and_filters() {
    let h = Harness::new().await;
    let admin = h.user("root@x.io", true).await;
    let plain = h.user("alice@x.io", false).await;
    let (_, at) = h.good_session(admin).await;
    let (_, pt) = h.good_session(plain).await;
    let (s, v) = call(&h, &pt, Method::GET, "/v1/admin/users", None).await;
    assert_eq!((s, code(&v)), (403, "instance_admin_required"));
    let org = h.org().await;
    h.member(org, admin, "owner", "active").await;
    let (_, key) = h.api_key(admin, org, None, false).await;
    let b = bearer(&key);
    let (s, _, v) = h
        .send(
            Method::GET,
            "/v1/admin/users",
            &[("authorization", &b)],
            None,
        )
        .await;
    assert_eq!((s.as_u16(), code(&v)), (403, "session_required"));

    let (s, v) = call(&h, &at, Method::GET, "/v1/admin/users", None).await;
    assert_eq!(s, 200);
    assert_eq!(v["items"].as_array().unwrap().len(), 2);
    assert!(v["items"][0].get("disabled_at").is_some());
    let (_, v) = call(&h, &at, Method::GET, "/v1/admin/users?q=ALIC", None).await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    let (_, v) = call(&h, &at, Method::GET, "/v1/admin/users?q=%25", None).await;
    assert_eq!(v["items"].as_array().unwrap().len(), 0);
    let (_, v) = call(&h, &at, Method::GET, "/v1/admin/users?limit=1", None).await;
    assert!(v["next_cursor"].is_string());
}

#[tokio::test]
async fn patch_user_disable_revokes_and_self_rejected() {
    let h = Harness::new().await;
    let admin = h.user("root@x.io", true).await;
    let u = h.user("u@x.io", false).await;
    let (_, at) = h.good_session(admin).await;
    let (_, ut) = h.good_session(u).await;
    let (s, _) = call(&h, &ut, Method::GET, "/v1/me", None).await;
    assert_eq!(s, 200);
    let uri = format!("/v1/admin/users/{u}");
    let (s, v) = call(
        &h,
        &at,
        Method::PATCH,
        &uri,
        Some(json!({"disabled": true})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert!(v["disabled_at"].is_string());
    let (s, _) = call(&h, &ut, Method::GET, "/v1/me", None).await;
    assert_eq!(s, 401);
    let (s, v) = call(
        &h,
        &at,
        Method::PATCH,
        &uri,
        Some(json!({"disabled": false, "is_instance_admin": true})),
    )
    .await;
    assert_eq!(s, 200);
    assert!(v["disabled_at"].is_null());
    assert_eq!(v["is_instance_admin"], true);
    let own = format!("/v1/admin/users/{admin}");
    let (s, v) = call(
        &h,
        &at,
        Method::PATCH,
        &own,
        Some(json!({"disabled": true})),
    )
    .await;
    assert_eq!((s, code(&v)), (422, "cannot_modify_self"));
    let missing = format!("/v1/admin/users/{}", uuid::Uuid::now_v7());
    let (s, _) = call(&h, &at, Method::PATCH, &missing, Some(json!({}))).await;
    assert_eq!(s, 404);
}

/// RFE-004: `bootstrap-admin` pre-provisions a verified, credential-less admin that binds at the
/// first OIDC sign-in; no secret is printed.
#[tokio::test]
async fn bootstrap_admin_preprovisions_and_binds_at_first_sign_in() {
    use aurora_api::domains::admin::service;
    // invite_only (the default): nobody else could register.
    let h = Harness::new().await;
    let url = service::bootstrap_admin(h.pool(), "http://localhost:3000/", true, "New@X.io")
        .await
        .unwrap();
    assert_eq!(url, "http://localhost:3000/auth/login");
    let row: (bool, bool, bool) = sqlx::query_as(
        "SELECT is_instance_admin, email_verified_at IS NOT NULL, password_hash IS NULL
         FROM users WHERE email = 'new@x.io'",
    )
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert_eq!(row, (true, true, true));
    let ids: i64 = sqlx::query_scalar("SELECT count(*) FROM user_identities")
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(ids, 0, "no identity until the first sign-in");

    // The first OIDC sign-in with that verified e-mail binds the identity to that same user.
    let s = h.sign_in("new@x.io").await;
    let ids: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_identities WHERE user_id = $1 AND provider_id = 'dev'",
    )
    .bind(s.user_id)
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert_eq!(ids, 1);
    let (st, me) = h.get("/v1/me", &[("cookie", &s.cookie)]).await;
    assert_eq!(st.as_u16(), 200);
    assert_eq!(me["user"]["is_instance_admin"], true);
    // Someone else cannot take the pre-provisioned account by another e-mail.
    assert_eq!(
        h.try_sign_in("dev", "someone@x.io").await.err().as_deref(),
        Some("signup_closed")
    );

    // re-running is idempotent; revoke works
    service::bootstrap_admin(h.pool(), "http://localhost:3000", true, "new@x.io")
        .await
        .unwrap();
    assert!(service::revoke_admin(h.pool(), "new@x.io").await.unwrap());
    assert!(!service::revoke_admin(h.pool(), "ghost@x.io").await.unwrap());
    let flag: bool =
        sqlx::query_scalar("SELECT is_instance_admin FROM users WHERE email = 'new@x.io'")
            .fetch_one(h.pool())
            .await
            .unwrap();
    assert!(!flag);
    // Refused when no OIDC provider is configured, and for a malformed address.
    let e = service::bootstrap_admin(h.pool(), "http://localhost:3000", false, "a@x.io")
        .await
        .unwrap_err();
    assert_eq!(e.code(), "no_oidc_provider");
    assert!(
        service::bootstrap_admin(h.pool(), "http://localhost:3000", true, "nope")
            .await
            .is_err()
    );
}
