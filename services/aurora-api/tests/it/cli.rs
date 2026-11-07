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
