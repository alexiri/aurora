#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! SEC-70 log half (P4-MCON-1): managed connector secret and generated values never reach the
//! logs. Own test binary with a single test: it installs a scoped tracing subscriber, which must
//! not race with other tests' callsite registration (same reason as `sec10_logs.rs`).

#[path = "it/common/mod.rs"]
mod common;
#[path = "it/managed_support.rs"]
mod managed_support;

use crate::managed_support::*;
use http::Method;
use serde_json::json;
use uuid::Uuid;

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

/// SEC-70: across create, PATCH (replace, regenerate), the host's assignments and status calls,
/// and a store failure, the plaintext of a supplied or generated secret is never logged.
#[tokio::test]
async fn sec70_secret_values_never_reach_the_logs() {
    let buf = LogBuf::default();
    let _g = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(buf.clone())
            .finish(),
    );
    // Other tests in this binary may have cached "disabled" interest for callsites.
    tracing::callsite::rebuild_interest_cache();

    let m = M::with_memory_store().await;
    let mem = m.mem.clone().unwrap();
    let host = Uuid::now_v7();
    m.host_register(host, "1.0.0", &[&manifest("alpha")]).await;
    let (s, v) = m
        .admin(
            Method::POST,
            "/v1/admin/connectors",
            Some(m.create_body("alpha")),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = m
        .admin(
            Method::PATCH,
            "/v1/admin/connectors/alpha",
            Some(json!({"secrets": {"api_key": SECRET_VALUE_2}, "regenerate": ["callback_hmac_key"]})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, _, a) = m.assignments(Some(host), None, 0).await;
    assert_eq!(s, 200, "{a}");
    let generated = a["connectors"][0]["secrets"]["callback_hmac_key"]
        .as_str()
        .unwrap()
        .to_owned();
    m.host_req(
        None,
        Method::POST,
        "/status",
        Some(json!({"instance_id": host, "connectors": [{"id": "alpha", "state": "running"}]})),
    )
    .await;
    // Failures log too.
    mem.set_unavailable(true);
    let (s, _) = m
        .admin(
            Method::PATCH,
            "/v1/admin/connectors/alpha",
            Some(json!({"secrets": {"api_key": SECRET_VALUE}})),
        )
        .await;
    assert_eq!(s, 503);
    let (s, _, _) = m.assignments(Some(host), None, 0).await;
    assert_eq!(s, 503);
    mem.set_unavailable(false);
    m.admin(Method::DELETE, "/v1/admin/connectors/alpha", None)
        .await;

    let logs = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    assert!(!logs.is_empty(), "the subscriber captured the run");
    for needle in [SECRET_VALUE, SECRET_VALUE_2, generated.as_str()] {
        assert!(!logs.contains(needle), "a secret value reached the logs");
    }
}
