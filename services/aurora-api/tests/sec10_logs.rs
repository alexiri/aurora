#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! SEC-10 log-scrub test. Own test binary with a single test: it installs a scoped tracing
//! subscriber, which must not race with other tests' callsite registration.
mod common;

use common::*;
use http::Method;
use serde_json::json;

async fn sess(
    h: &Harness,
    tok: &str,
    m: Method,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    let c = cookie(tok);
    let (s, _, v) = h
        .send(m, uri, &[("cookie", &c), ("origin", ORIGIN)], body)
        .await;
    (s.as_u16(), v)
}
async fn key(
    h: &Harness,
    tok: &str,
    m: Method,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    let b = bearer(tok);
    let (s, _, v) = h.send(m, uri, &[("authorization", &b)], body).await;
    (s.as_u16(), v)
}

#[derive(Clone, Default)]
struct LogBuf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for LogBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuf {
    type Writer = LogBuf;
    fn make_writer(&'a self) -> LogBuf {
        self.clone()
    }
}

/// SEC-10: a capability token is stored only as its SHA-256 hash, shown once, and
/// never reaches the logs (captured at TRACE across create, use, rejection).
#[tokio::test]
async fn sec10_token_is_hash_only_and_never_logged() {
    use sha2::{Digest, Sha256};
    let buf = LogBuf::default();
    let _g = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(buf.clone())
            .finish(),
    );
    // Other tests in this binary may have cached "disabled" interest for callsites.
    tracing::callsite::rebuild_interest_cache();
    let h = &Harness::new().await;
    let org = h.org().await;
    let owner = h.user("o@x.io", false).await;
    let member = h.user("m@x.io", false).await;
    h.member(org, owner, "owner", "active").await;
    h.member(org, member, "member", "active").await;
    let ws = h.workspace(org).await;
    let (_, owner_tok) = h.good_session(owner).await;
    let (_, member_tok) = h.good_session(member).await;
    let u = format!("/v1/workspaces/{ws}/api-keys");
    let (s, v) = sess(
        h,
        &owner_tok,
        Method::POST,
        &u,
        Some(json!({"name":"logs"})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let k = v["key"].as_str().unwrap().to_owned();
    let stored: Vec<u8> = sqlx::query_scalar("SELECT token_hash FROM api_keys WHERE name='logs'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(
        stored,
        Sha256::digest(k.as_bytes()).to_vec(),
        "SHA-256 of the token"
    );
    // Use the key, then present a corrupted copy.
    let (s, _) = key(h, &k, Method::GET, &u, None).await;
    assert_eq!(s, 200);
    let bad = format!("{}x", &k[..k.len() - 1]);
    let (s, _) = key(h, &bad, Method::GET, &u, None).await;
    assert_eq!(s, 401);
    let logs = String::from_utf8_lossy(&buf.0.lock().unwrap()).into_owned();
    assert!(
        !logs.is_empty(),
        "the capture must be live, or the check is vacuous"
    );
    for secret in [
        k.as_str(),
        bad.as_str(),
        owner_tok.as_str(),
        member_tok.as_str(),
    ] {
        assert!(!logs.contains(secret), "token leaked into logs");
    }
    // Shown once: no read path returns it again.
    let (_, v) = sess(h, &owner_tok, Method::GET, &u, None).await;
    assert!(!v.to_string().contains(&k));
}
