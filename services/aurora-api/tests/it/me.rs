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

/// `GET /v1/me` from `peer`, sending `xff` when given.
async fn me_from(h: &Harness, tok: &str, peer: &str, xff: Option<&str>) -> Value {
    use axum::extract::ConnectInfo;
    use tower::ServiceExt;
    let mut b = http::Request::builder()
        .method(Method::GET)
        .uri("/v1/me")
        .header("cookie", cookie(tok));
    if let Some(x) = xff {
        b = b.header("x-forwarded-for", x);
    }
    let mut req = b.body(axum::body::Body::empty()).unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(std::net::SocketAddr::new(
            peer.parse().unwrap(),
            40000,
        )));
    let res = h.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), 200);
    use http_body_util::BodyExt;
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

/// FU-P2-025b (C-195): `client_address` is the address as the API resolves it behind
/// `server.trusted_proxies`; an untrusted peer cannot choose it with `X-Forwarded-For`.
#[tokio::test]
async fn me_reports_the_callers_address_as_the_api_sees_it() {
    let h =
        Harness::with_config(|c| c.server.trusted_proxies = vec!["10.0.0.0/8".to_owned()]).await;
    let u = h.user("addr@x.io", false).await;
    let (_, tok) = h.good_session(u).await;
    // Direct peer, no header.
    let v = me_from(&h, &tok, "203.0.113.7", None).await;
    assert_eq!(v["client_address"], "203.0.113.7");
    // Trusted proxy: the rightmost untrusted hop (a spoofed left entry is ignored).
    let v = me_from(
        &h,
        &tok,
        "10.1.2.3",
        Some("6.6.6.6, 198.51.100.9, 10.9.9.9"),
    )
    .await;
    assert_eq!(v["client_address"], "198.51.100.9");
    // Trusted proxy without the header: the proxy itself.
    let v = me_from(&h, &tok, "10.1.2.3", None).await;
    assert_eq!(v["client_address"], "10.1.2.3");
    // Untrusted peer sending X-Forwarded-For: ignored.
    let v = me_from(&h, &tok, "203.0.113.7", Some("1.2.3.4")).await;
    assert_eq!(v["client_address"], "203.0.113.7");
    // No socket info (in-process call): null, field present.
    let (s, v) = call(&h, &tok, Method::GET, "/v1/me", None).await;
    assert_eq!(s, 200);
    assert!(v.get("client_address").is_some_and(Value::is_null), "{v}");
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

async fn stamp_reauth(h: &Harness, sid: Uuid, ago_secs: i64) {
    sqlx::query("UPDATE sessions SET reauth_at = now() - make_interval(secs => $2) WHERE id = $1")
        .bind(sid)
        .bind(ago_secs as f64)
        .execute(h.pool())
        .await
        .unwrap();
}

/// RFE-004: account deletion needs an OIDC re-authentication of the session within 10 minutes
/// (412 `reauth_required`); there is no password to supply any more.
#[tokio::test]
async fn delete_me_needs_recent_reauth() {
    let h = Harness::new().await;
    let u = h.user("d@x.io", false).await;
    let (sid, tok) = h.good_session(u).await;
    let org = h.org().await;
    h.member(org, u, "owner", "active").await;
    // never re-authenticated
    let (s, v) = call(&h, &tok, Method::DELETE, "/v1/me", None).await;
    assert_eq!((s, code(&v)), (412, "reauth_required"));
    // a body (the phase-0 password) does not help
    let (s, v) = call(
        &h,
        &tok,
        Method::DELETE,
        "/v1/me",
        Some(json!({"password": "correct horse battery"})),
    )
    .await;
    assert_eq!((s, code(&v)), (412, "reauth_required"));
    // stale re-authentication (11 min)
    stamp_reauth(&h, sid, 660).await;
    let (s, v) = call(&h, &tok, Method::DELETE, "/v1/me", None).await;
    assert_eq!((s, code(&v)), (412, "reauth_required"));
    // fresh, but last owner
    stamp_reauth(&h, sid, 5).await;
    let (s, v) = call(&h, &tok, Method::DELETE, "/v1/me", None).await;
    assert_eq!((s, code(&v)), (409, "last_owner"));
    // second owner: ok
    let o2 = h.user("o2@x.io", false).await;
    h.member(org, o2, "owner", "active").await;
    let ws = h.workspace(org).await;
    h.bind_ws(ws, u, "owner").await;
    let (s, _) = call(&h, &tok, Method::DELETE, "/v1/me", None).await;
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

/// Another session of the same user does not inherit a re-authentication.
#[tokio::test]
async fn reauth_is_per_session() {
    let h = Harness::new().await;
    let u = h.user("p@x.io", false).await;
    let (s1, t1) = h.good_session(u).await;
    let (_, t2) = h.good_session(u).await;
    stamp_reauth(&h, s1, 1).await;
    let (s, v) = call(&h, &t2, Method::DELETE, "/v1/me", None).await;
    assert_eq!((s, code(&v)), (412, "reauth_required"));
    let (s, _) = call(&h, &t1, Method::DELETE, "/v1/me", None).await;
    assert_eq!(s, 204);
}

/// FIX-063 (C-465): a suspended membership has no effective role, even for an owner.
#[tokio::test]
async fn effective_role_is_null_for_a_suspended_membership() {
    let h = Harness::new().await;
    let u = h.user("s@x.io", false).await;
    let (_, tok) = h.good_session(u).await;
    let o1 = h.org().await;
    let o2 = h.org().await;
    h.member(o1, u, "owner", "active").await;
    h.member(o2, u, "owner", "suspended").await;
    let (s, v) = call(&h, &tok, Method::GET, "/v1/me", None).await;
    assert_eq!(s, 200, "{v}");
    for m in v["memberships"].as_array().unwrap() {
        if m["org"]["id"] == json!(o2) {
            assert_eq!(m["role"], "owner");
            assert!(m["effective_role"].is_null(), "{m}");
        } else {
            assert_eq!(m["effective_role"], "owner");
        }
    }
}
