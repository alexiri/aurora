#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use http::Method;
use serde_json::{Value, json};

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

async fn post(h: &Harness, uri: &str, tok: Option<&str>, body: Value) -> (u16, Value) {
    let c = tok.map(cookie);
    let mut hd = vec![("origin", ORIGIN)];
    if let Some(c) = c.as_deref() {
        hd.push(("cookie", c));
    }
    let (s, _, v) = h.send(Method::POST, uri, &hd, Some(body)).await;
    (s.as_u16(), v)
}

#[tokio::test]
async fn happy_path_once_and_slow_down() {
    let h = Harness::new().await;
    let org = h.org().await;
    let u = h.user("c@x.io", false).await;
    h.member(org, u, "member", "active").await;
    let ws = h.workspace(org).await;
    h.bind_ws(ws, u, "member").await;
    let (_, tok) = h.good_session(u).await;

    let (s, v) = post(&h, "/v1/auth/cli", None, json!({"hostname":"my laptop"})).await;
    assert_eq!(s, 200, "{v}");
    let dc = v["device_code"].as_str().unwrap().to_owned();
    let uc = v["user_code"].as_str().unwrap().to_owned();
    assert_eq!(uc.len(), 9);
    assert_eq!(v["interval"], 5);
    assert_eq!(v["expires_in"], 600);
    assert!(
        v["verification_url"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/auth/cli?code={uc}"))
    );

    let (s, v) = post(&h, "/v1/auth/cli/token", None, json!({"device_code":dc})).await;
    assert_eq!((s, code(&v)), (400, "authorization_pending"));
    let (s, v) = post(&h, "/v1/auth/cli/token", None, json!({"device_code":dc})).await;
    assert_eq!((s, code(&v)), (400, "slow_down"));

    // key callers cannot approve
    let (_, k) = h.api_key(u, org, Some(ws), false).await;
    let b = bearer(&k);
    let (st, _, v) = h
        .send(
            Method::POST,
            "/v1/auth/cli/approve",
            &[("authorization", &b)],
            Some(json!({"user_code":uc,"workspace_id":ws})),
        )
        .await;
    assert_eq!((st.as_u16(), code(&v)), (403, "session_required"));
    // invisible workspace -> 404
    let other = h.workspace(h.org().await).await;
    let (s, _) = post(
        &h,
        "/v1/auth/cli/approve",
        Some(&tok),
        json!({"user_code":uc,"workspace_id":other}),
    )
    .await;
    assert_eq!(s, 404);
    // unknown code -> 404
    let (s, _) = post(
        &h,
        "/v1/auth/cli/approve",
        Some(&tok),
        json!({"user_code":"AAAA-AAAA","workspace_id":ws}),
    )
    .await;
    assert_eq!(s, 404);

    let (s, _) = post(
        &h,
        "/v1/auth/cli/approve",
        Some(&tok),
        json!({"user_code":uc.to_lowercase(),"workspace_id":ws}),
    )
    .await;
    assert_eq!(s, 204);
    let (s, v) = post(&h, "/v1/auth/cli/token", None, json!({"device_code":dc})).await;
    assert_eq!(s, 200, "{v}");
    let key = v["api_key"].as_str().unwrap().to_owned();
    assert!(key.starts_with("auk_"));
    assert_eq!(v["workspace"]["id"], json!(ws));
    let name: String = sqlx::query_scalar("SELECT name FROM api_keys WHERE token_hash = $1")
        .bind(&aurora_core::token::hash(&key)[..])
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(name, "CLI on my laptop");
    // (FIX-108, D-178 Q-R5-16) The minted key expires in 90 days by default.
    let in_window: bool = sqlx::query_scalar(
        "SELECT expires_at > now() + interval '89 days' AND expires_at < now() + interval '91 days'
         FROM api_keys WHERE token_hash = $1",
    )
    .bind(&aurora_core::token::hash(&key)[..])
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert!(in_window, "CLI keys expire after 90 days by default");
    let (s, v) = post(&h, "/v1/auth/cli/token", None, json!({"device_code":dc})).await;
    assert_eq!((s, code(&v)), (400, "invalid_token"));
    let (s, v) = post(
        &h,
        "/v1/auth/cli/token",
        None,
        json!({"device_code":"nope"}),
    )
    .await;
    assert_eq!((s, code(&v)), (400, "invalid_token"));
    // minted key works
    let (s, _) = h.get("/v1/me", &[("authorization", &bearer(&key))]).await;
    assert_eq!(s, 200);
}

/// (FIX-108, AUTH-5) The approving session sees what it is approving: the requesting host name,
/// the start time, the coarse address and the key lifetime; nothing else may read it.
#[tokio::test]
async fn pending_login_detail_is_shown_to_the_approving_session() {
    let h = Harness::new().await;
    let org = h.org().await;
    let u = h.user("p@x.io", false).await;
    h.member(org, u, "member", "active").await;
    let (_, tok) = h.good_session(u).await;
    let (_, v) = post(&h, "/v1/auth/cli", None, json!({"hostname":"build-box"})).await;
    let uc = v["user_code"].as_str().unwrap().to_owned();
    sqlx::query("UPDATE cli_logins SET requested_addr = '203.0.113.0/24'")
        .execute(h.pool())
        .await
        .unwrap();
    let uri = format!("/v1/auth/cli/pending?code={}", uc.to_lowercase());

    let (s, v) = h.get(&uri, &[("cookie", &cookie(&tok))]).await;
    assert_eq!(s.as_u16(), 200, "{v}");
    assert_eq!(v["hostname"], "build-box");
    assert_eq!(v["address"], "203.0.113.0/24");
    assert_eq!(v["key_expires_in_days"], 90);
    assert!(v["requested_at"].as_str().is_some() && v["expires_at"].as_str().is_some());

    // No session: 401; a key: session_required; unknown code: 404; malformed: 400.
    let (s, _) = h.get(&uri, &[]).await;
    assert_eq!(s.as_u16(), 401);
    let (_, k) = h.api_key(u, org, None, false).await;
    let (s, v) = h.get(&uri, &[("authorization", &bearer(&k))]).await;
    assert_eq!((s.as_u16(), code(&v)), (403, "session_required"));
    let (s, _) = h
        .get(
            "/v1/auth/cli/pending?code=AAAA-AAAA",
            &[("cookie", &cookie(&tok))],
        )
        .await;
    assert_eq!(s.as_u16(), 404);
    let (s, _) = h
        .get("/v1/auth/cli/pending?code=x", &[("cookie", &cookie(&tok))])
        .await;
    assert_eq!(s.as_u16(), 400);
}

#[tokio::test]
async fn expiry() {
    let h = Harness::new().await;
    let (s, v) = post(&h, "/v1/auth/cli", None, json!({})).await;
    assert_eq!(s, 200);
    let dc = v["device_code"].as_str().unwrap();
    sqlx::query("UPDATE cli_logins SET expires_at = now() - interval '1 second'")
        .execute(h.pool())
        .await
        .unwrap();
    let (s, v) = post(&h, "/v1/auth/cli/token", None, json!({"device_code":dc})).await;
    assert_eq!((s, code(&v)), (400, "expired_token"));
}
