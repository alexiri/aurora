#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod common;

use common::*;
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

#[tokio::test]
async fn bootstrap_admin_link_sets_password_and_revoke() {
    use aurora_api::domains::admin::service;
    let h = Harness::new().await;
    let link = service::bootstrap_admin(h.pool(), "http://localhost:3000/", "New@X.io")
        .await
        .unwrap();
    assert!(
        link.starts_with("http://localhost:3000/auth/reset?token=aui_"),
        "{link}"
    );
    let tok = link.split("token=").nth(1).unwrap();
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/auth/password/reset",
            &[("origin", ORIGIN)],
            Some(json!({"token": tok, "password": "correct horse battery"})),
        )
        .await;
    assert_eq!(s.as_u16(), 204, "{v}");
    let (s, hdrs, _) = h
        .send(
            Method::POST,
            "/v1/auth/login",
            &[("origin", ORIGIN)],
            Some(json!({"email": "new@x.io", "password": "correct horse battery"})),
        )
        .await;
    assert_eq!(s.as_u16(), 200);
    assert!(hdrs.contains_key("set-cookie"));
    // re-running promotes/rotates without error
    service::bootstrap_admin(h.pool(), "http://localhost:3000", "new@x.io")
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
}
