#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

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

async fn set_password(h: &Harness, user: Uuid, pw: &str) {
    let hash = aurora_api::domains::auth::password::hash(pw.to_owned())
        .await
        .unwrap();
    sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
        .bind(user)
        .bind(hash)
        .execute(h.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn me_shape_and_suspended_membership() {
    let h = Harness::new().await;
    let u = h.user("m@x.io", false).await;
    let (_, tok) = h.good_session(u).await;
    let o1 = h.org().await;
    let o2 = h.org().await;
    h.member(o1, u, "member", "active").await;
    h.member(o2, u, "member", "suspended").await;
    let w1 = h.workspace(o1).await;
    let _w1b = h.workspace(o1).await;
    let w2 = h.workspace(o2).await;
    h.bind_ws(w1, u, "member").await;
    h.bind_ws(w2, u, "member").await;
    let (s, v) = call(&h, &tok, Method::GET, "/v1/me", None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["user"]["email"], "m@x.io");
    let ms = v["memberships"].as_array().unwrap();
    assert_eq!(ms.len(), 2);
    for m in ms {
        if m["org"]["id"] == json!(o1) {
            assert_eq!(m["status"], "active");
            assert_eq!(m["workspaces"].as_array().unwrap().len(), 1);
            assert_eq!(m["workspaces"][0]["my_role"], "member");
        } else {
            assert_eq!(m["status"], "suspended");
            assert!(m["workspaces"].as_array().unwrap().is_empty());
        }
    }
    // org admin sees all workspaces as owner
    let a = h.user("a@x.io", false).await;
    let (_, at) = h.good_session(a).await;
    h.member(o1, a, "admin", "active").await;
    let (_, v) = call(&h, &at, Method::GET, "/v1/me", None).await;
    let ws = v["memberships"][0]["workspaces"].as_array().unwrap();
    assert_eq!(ws.len(), 2);
    assert!(ws.iter().all(|w| w["my_role"] == "owner"));
}

#[tokio::test]
async fn patch_me_validation() {
    let h = Harness::new().await;
    let u = h.user("p@x.io", false).await;
    let (_, tok) = h.good_session(u).await;
    let o1 = h.org().await;
    let o2 = h.org().await;
    let o3 = h.org().await;
    h.member(o1, u, "member", "active").await;
    h.member(o2, u, "member", "suspended").await;
    let (s, v) = call(
        &h,
        &tok,
        Method::PATCH,
        "/v1/me",
        Some(json!({"display_name": "New"})),
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(v["user"]["display_name"], "New");
    let (s, _) = call(
        &h,
        &tok,
        Method::PATCH,
        "/v1/me",
        Some(json!({"display_name": ""})),
    )
    .await;
    assert_eq!(s, 400);
    let long = "x".repeat(81);
    let (s, _) = call(
        &h,
        &tok,
        Method::PATCH,
        "/v1/me",
        Some(json!({"display_name": long})),
    )
    .await;
    assert_eq!(s, 400);
    for bad in [o2, o3] {
        let (s, _) = call(
            &h,
            &tok,
            Method::PATCH,
            "/v1/me",
            Some(json!({"default_org_id": bad})),
        )
        .await;
        assert_eq!(s, 404);
    }
    let (s, v) = call(
        &h,
        &tok,
        Method::PATCH,
        "/v1/me",
        Some(json!({"default_org_id": o1})),
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(v["default_org_id"], json!(o1));
}

#[tokio::test]
async fn delete_me_rules() {
    let h = Harness::new().await;
    let u = h.user("d@x.io", false).await;
    set_password(&h, u, "correct horse battery").await;
    let (sid, tok) = h.good_session(u).await;
    let org = h.org().await;
    h.member(org, u, "owner", "active").await;
    // no / wrong password
    let (s, v) = call(&h, &tok, Method::DELETE, "/v1/me", Some(json!({}))).await;
    assert_eq!((s, code(&v)), (403, "reauthentication_required"));
    let (s, v) = call(
        &h,
        &tok,
        Method::DELETE,
        "/v1/me",
        Some(json!({"password": "nope nope nope"})),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "reauthentication_required"));
    // last owner
    let good = json!({"password": "correct horse battery"});
    let (s, v) = call(&h, &tok, Method::DELETE, "/v1/me", Some(good.clone())).await;
    assert_eq!((s, code(&v)), (409, "last_owner"));
    // second owner: ok
    let o2 = h.user("o2@x.io", false).await;
    h.member(org, o2, "owner", "active").await;
    let ws = h.workspace(org).await;
    h.bind_ws(ws, u, "owner").await;
    let (s, _) = call(&h, &tok, Method::DELETE, "/v1/me", Some(good)).await;
    assert_eq!(s, 204);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE id = $1")
        .bind(u)
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE id = $1")
        .bind(sid)
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
    let (s, _) = call(&h, &tok, Method::GET, "/v1/me", None).await;
    assert_eq!(s, 401);
}

#[tokio::test]
async fn delete_me_sso_only_needs_recent_session() {
    let h = Harness::new().await;
    let u = h.user("s@x.io", false).await;
    let (sid, tok) = h.good_session(u).await;
    sqlx::query("UPDATE sessions SET created_at = now() - interval '1 hour' WHERE id = $1")
        .bind(sid)
        .execute(h.pool())
        .await
        .unwrap();
    let (s, v) = call(&h, &tok, Method::DELETE, "/v1/me", Some(json!({}))).await;
    assert_eq!((s, code(&v)), (403, "reauthentication_required"));
    sqlx::query("UPDATE sessions SET created_at = now() WHERE id = $1")
        .bind(sid)
        .execute(h.pool())
        .await
        .unwrap();
    let (s, _) = call(&h, &tok, Method::DELETE, "/v1/me", Some(json!({}))).await;
    assert_eq!(s, 204);
}
