#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

async fn sess(h: &Harness, tok: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
    let c = cookie(tok);
    let (s, _, v) = h
        .send(m, uri, &[("cookie", &c), ("origin", ORIGIN)], body)
        .await;
    (s.as_u16(), v)
}
async fn key(h: &Harness, tok: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
    let b = bearer(tok);
    let (s, _, v) = h.send(m, uri, &[("authorization", &b)], body).await;
    (s.as_u16(), v)
}
fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

struct World {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    ws2: Uuid,
    owner: Uuid,
    owner_tok: String,
    member: Uuid,
    member_tok: String,
}

async fn world() -> World {
    let h = Harness::new().await;
    let org = h.org().await;
    let owner = h.user("o@x.io", false).await;
    let member = h.user("m@x.io", false).await;
    h.member(org, owner, "owner", "active").await;
    h.member(org, member, "member", "active").await;
    let ws = h.workspace(org).await;
    let ws2 = h.workspace(org).await;
    h.bind_ws(ws, member, "member").await;
    let (_, owner_tok) = h.good_session(owner).await;
    let (_, member_tok) = h.good_session(member).await;
    World {
        h,
        org,
        ws,
        ws2,
        owner,
        owner_tok,
        member,
        member_tok,
    }
}

#[tokio::test]
async fn create_rules_and_shown_once() {
    let w = world().await;
    let h = &w.h;
    let u = format!("/v1/workspaces/{}/api-keys", w.ws);
    let (s, v) = sess(
        h,
        &w.owner_tok,
        Method::POST,
        &u,
        Some(json!({"name":"ci"})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let k = v["key"].as_str().unwrap().to_owned();
    assert!(k.starts_with("auk_"));
    assert_eq!(v["api_key"]["workspace_id"], json!(w.ws));
    assert!(v["api_key"].get("key").is_none());
    // DB stores hash + 12 char prefix only.
    let (p, hash): (String, Vec<u8>) =
        sqlx::query_as("SELECT display_prefix, token_hash FROM api_keys WHERE name='ci'")
            .fetch_one(h.pool())
            .await
            .unwrap();
    assert_eq!(p.len(), 12);
    assert_eq!(hash.len(), 32);
    // list never contains the token
    let (s, v) = sess(h, &w.owner_tok, Method::GET, &u, None).await;
    assert_eq!(s, 200);
    assert!(!v.to_string().contains(&k));
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    // creating via API key -> session_required
    let (s, v) = key(h, &k, Method::POST, &u, Some(json!({"name":"x"}))).await;
    assert_eq!((s, code(&v)), (403, "session_required"));
    // member cannot create workspace key
    let (s, v) = sess(
        h,
        &w.member_tok,
        Method::POST,
        &u,
        Some(json!({"name":"x"})),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "forbidden"));
    // validation
    let (s, _) = sess(h, &w.owner_tok, Method::POST, &u, Some(json!({"name":""}))).await;
    assert_eq!(s, 400);
    let (s, _) = sess(
        h,
        &w.owner_tok,
        Method::POST,
        &u,
        Some(json!({"name":"a","expires_at":"2000-01-01T00:00:00Z"})),
    )
    .await;
    assert_eq!(s, 400);
    // org key
    let ou = format!("/v1/orgs/{}/api-keys", w.org);
    let (s, v) = sess(
        h,
        &w.owner_tok,
        Method::POST,
        &ou,
        Some(json!({"name":"org"})),
    )
    .await;
    assert_eq!(s, 201);
    assert!(v["key"].as_str().unwrap().starts_with("auo_"));
    assert!(v["api_key"]["workspace_id"].is_null());
    let (s, _) = sess(
        h,
        &w.member_tok,
        Method::POST,
        &ou,
        Some(json!({"name":"org"})),
    )
    .await;
    assert_eq!(s, 403);
    let (s, v) = sess(h, &w.owner_tok, Method::GET, &ou, None).await;
    assert_eq!((s, v["items"].as_array().unwrap().len()), (200, 1));
}

#[tokio::test]
async fn confinement_revoke_and_suspension() {
    let w = world().await;
    let h = &w.h;
    let u = format!("/v1/workspaces/{}/api-keys", w.ws);
    let (_, v) = sess(
        h,
        &w.owner_tok,
        Method::POST,
        &u,
        Some(json!({"name":"ci"})),
    )
    .await;
    let k = v["key"].as_str().unwrap().to_owned();
    let id = v["api_key"]["id"].as_str().unwrap().to_owned();
    // key usable on its workspace, mismatch elsewhere
    let (s, _) = key(h, &k, Method::GET, &format!("/t/ws/{}", w.ws), None).await;
    assert_eq!(s, 200);
    let (s, v) = key(h, &k, Method::GET, &format!("/t/ws/{}", w.ws2), None).await;
    assert_eq!((s, code(&v)), (403, "workspace_mismatch"));
    // member (non-owner of key, not can_own) cannot revoke
    let (s, _) = sess(
        h,
        &w.member_tok,
        Method::DELETE,
        &format!("/v1/api-keys/{id}"),
        None,
    )
    .await;
    assert_eq!(s, 403);
    // outsider sees 404
    let out = h.user("out@x.io", false).await;
    let (_, ot) = h.good_session(out).await;
    let (s, _) = sess(h, &ot, Method::DELETE, &format!("/v1/api-keys/{id}"), None).await;
    assert_eq!(s, 404);
    // warm cache, then revoke -> immediately 401
    let (s, _) = key(h, &k, Method::GET, "/v1/me", None).await;
    assert_eq!(s, 200);
    let (s, _) = sess(
        h,
        &w.owner_tok,
        Method::DELETE,
        &format!("/v1/api-keys/{id}"),
        None,
    )
    .await;
    assert_eq!(s, 204);
    // cache invalidation is via NOTIFY; flush like the listener would
    h.state
        .session_cache
        .invalidate(&json!({"kind":"key","id":id}));
    let (s, _) = key(h, &k, Method::GET, "/v1/me", None).await;
    assert_eq!(s, 401);
    let (_, v) = sess(h, &w.owner_tok, Method::GET, &u, None).await;
    assert!(v["items"][0]["revoked_at"].is_string());

    // owner suspension kills key
    let (_, mk) = h.api_key(w.member, w.org, Some(w.ws), false).await;
    let (s, _) = key(h, &mk, Method::GET, "/v1/me", None).await;
    assert_eq!(s, 200);
    h.set_member_status(w.org, w.member, "suspended").await;
    h.state.session_cache.clear();
    let (s, _) = key(h, &mk, Method::GET, "/v1/me", None).await;
    assert_eq!(s, 401);
}

#[tokio::test]
async fn member_revokes_own_and_personal_list() {
    let w = world().await;
    let h = &w.h;
    let (mid, _mk) = h.api_key(w.member, w.org, Some(w.ws), false).await;
    let (_oid, _) = h.api_key(w.owner, w.org, Some(w.ws), false).await;
    let (s, v) = sess(h, &w.member_tok, Method::GET, "/v1/me/api-keys", None).await;
    assert_eq!(s, 200);
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], json!(mid));
    let (s, _) = key(h, &_mk, Method::GET, "/v1/me/api-keys", None).await;
    assert_eq!(s, 403);
    let (s, _) = sess(
        h,
        &w.member_tok,
        Method::DELETE,
        &format!("/v1/api-keys/{mid}"),
        None,
    )
    .await;
    assert_eq!(s, 204);
}

#[tokio::test]
async fn resolve_with_workspace_key() {
    let w = world().await;
    let h = &w.h;
    let (slug,): (String,) = sqlx::query_as("SELECT slug FROM organizations WHERE id=$1")
        .bind(w.org)
        .fetch_one(h.pool())
        .await
        .unwrap();
    let (wslug,): (String,) = sqlx::query_as("SELECT slug FROM workspaces WHERE id=$1")
        .bind(w.ws)
        .fetch_one(h.pool())
        .await
        .unwrap();
    let (w2slug,): (String,) = sqlx::query_as("SELECT slug FROM workspaces WHERE id=$1")
        .bind(w.ws2)
        .fetch_one(h.pool())
        .await
        .unwrap();
    let (_, k) = h.api_key(w.member, w.org, Some(w.ws), false).await;
    let (s, v) = key(
        h,
        &k,
        Method::GET,
        &format!("/v1/resolve?org={slug}&workspace={wslug}"),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["workspace"]["id"], json!(w.ws));
    let (s, _) = key(h, &k, Method::GET, &format!("/v1/resolve?org={slug}"), None).await;
    assert_eq!(s, 200);
    let (s, _) = key(
        h,
        &k,
        Method::GET,
        &format!("/v1/resolve?org={slug}&workspace={w2slug}"),
        None,
    )
    .await;
    assert_eq!(s, 404);
    let other = h.org().await;
    let (oslug,): (String,) = sqlx::query_as("SELECT slug FROM organizations WHERE id=$1")
        .bind(other)
        .fetch_one(h.pool())
        .await
        .unwrap();
    let (s, _) = key(
        h,
        &k,
        Method::GET,
        &format!("/v1/resolve?org={oslug}"),
        None,
    )
    .await;
    assert_eq!(s, 404);
}

async fn audit_count(h: &Harness, action: &str) -> i64 {
    for _ in 0..100 {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = $1")
            .bind(action)
            .fetch_one(h.pool())
            .await
            .unwrap();
        if n > 0 {
            return n;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    0
}

/// SEC-32: key creation and revocation are audited, and the audit row never carries
/// the token.
#[tokio::test]
async fn sec32_key_create_and_revoke_are_audited_without_the_token() {
    let w = world().await;
    let h = &w.h;
    let u = format!("/v1/workspaces/{}/api-keys", w.ws);
    let (_, v) = sess(
        h,
        &w.owner_tok,
        Method::POST,
        &u,
        Some(json!({"name":"aud"})),
    )
    .await;
    let k = v["key"].as_str().unwrap().to_owned();
    let id = v["api_key"]["id"].as_str().unwrap().to_owned();
    assert_eq!(audit_count(h, "api_key.create").await, 1);
    let (s, _) = sess(
        h,
        &w.owner_tok,
        Method::DELETE,
        &format!("/v1/api-keys/{id}"),
        None,
    )
    .await;
    assert_eq!(s, 204);
    assert_eq!(audit_count(h, "api_key.revoke").await, 1);
    let all: Vec<serde_json::Value> =
        sqlx::query_scalar("SELECT metadata FROM audit_log WHERE action LIKE 'api_key.%'")
            .fetch_all(h.pool())
            .await
            .unwrap();
    assert!(!all.iter().any(|m| m.to_string().contains(&k)));
}
