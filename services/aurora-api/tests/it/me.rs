#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use http::Method;
use serde_json::{Value, json};

async fn call(h: &Harness, tok: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
    h.call_tok(tok, m, uri, body).await
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

/// (P5-REFINE-6, C-1072) `can_create_org` says exactly when `POST /v1/orgs` works: an instance
/// admin, or anyone under `auth.org_creation = anyone`; a person already in an organization can
/// create another.
#[tokio::test]
async fn can_create_org_matches_post_orgs() {
    for anyone in [false, true] {
        let h = Harness::with_config(|c| {
            c.org_creation = if anyone {
                aurora_config::OrgCreation::Anyone
            } else {
                aurora_config::OrgCreation::InstanceAdmins
            };
        })
        .await;
        let org = h.org().await;
        for admin in [false, true] {
            let u = h
                .user(
                    &format!("p{}{}@x.io", u8::from(anyone), u8::from(admin)),
                    admin,
                )
                .await;
            h.member(org, u, "member", "active").await;
            let (_, tok) = h.good_session(u).await;
            let (s, me) = call(&h, &tok, Method::GET, "/v1/me", None).await;
            assert_eq!(s, 200, "{me}");
            let want = admin || anyone;
            assert_eq!(
                me["can_create_org"],
                json!(want),
                "anyone={anyone} admin={admin}"
            );
            let (s, v) = call(
                &h,
                &tok,
                Method::POST,
                "/v1/orgs",
                Some(json!({"name": format!("Second {}{}", u8::from(anyone), u8::from(admin))})),
            )
            .await;
            if want {
                assert_eq!(s, 201, "anyone={anyone} admin={admin}: {v}");
            } else {
                assert_eq!((s, code(&v)), (403, "org_creation_forbidden"), "{v}");
            }
        }
    }
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
    // (FIX-234) `/v1/me` lists only workspaces with own access (the view's law): the org
    // admin without a binding sees none here — they manage the org's workspaces from the org
    // pages (D-477).
    let a = h.user("a@x.io", false).await;
    let (_, at) = h.good_session(a).await;
    h.member(o1, a, "admin", "active").await;
    let (_, v) = call(&h, &at, Method::GET, "/v1/me", None).await;
    let m = v["memberships"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["org"]["id"] == json!(o1))
        .unwrap();
    assert!(m["workspaces"].as_array().unwrap().is_empty(), "{m}");
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

/// FIX-157 (D-317): account deletion needs no OIDC re-authentication: a session that never
/// re-authenticated is not asked for one (no 412); the last-owner rule still applies.
#[tokio::test]
async fn delete_me_needs_no_reauth() {
    let h = Harness::new().await;
    let u = h.user("d@x.io", false).await;
    let (sid, tok) = h.good_session(u).await;
    let org = h.org().await;
    h.member(org, u, "owner", "active").await;
    // never re-authenticated: the last-owner rule answers, not a re-authentication demand
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

/// FIX-157: account deletion stays session-only: an API key is refused and deletes nothing.
#[tokio::test]
async fn delete_me_refuses_an_api_key() {
    let h = Harness::new().await;
    let u = h.user("k@x.io", false).await;
    let org = h.org().await;
    h.member(org, u, "member", "active").await;
    let (_, key) = h.api_key(u, org, None, false).await;
    let (s, _, _) = h
        .send(
            Method::DELETE,
            "/v1/me",
            &[("authorization", &bearer(&key)), ("origin", ORIGIN)],
            None,
        )
        .await;
    assert!(s == 401 || s == 403, "api key got {s}");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE id = $1")
        .bind(u)
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(n, 1);
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
