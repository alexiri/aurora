#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-CNT / SEC-62: shared rate-limit counters in the API (docs/05 §Conventions *Shared
//! counters*): two replicas share one limit, an unavailable backend follows the class policy and
//! never loosens a limit, keys hold ids and keyed hashes only.

use crate::common::*;
use async_trait::async_trait;
use aurora_api::app::AppState;
use aurora_api::http::counters::ApiCounters;
use aurora_api::http::ratelimit::Bucket;
use aurora_counters::{
    BreakerConfig, BucketTake, ClassPolicy, CounterError, CounterStore, MemoryStore, Verdict,
    WindowProbe, WindowUnit,
};
use axum::Router;
use http::{HeaderMap, Method, StatusCode};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use uuid::Uuid;

/// A store that can be switched off (a stopped Redis / a broken pool).
struct Switch {
    down: AtomicBool,
    inner: Arc<MemoryStore>,
}

impl Switch {
    fn check(&self) -> Result<(), CounterError> {
        if self.down.load(Ordering::Relaxed) {
            Err(CounterError::Unavailable("stopped".into()))
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl CounterStore for Switch {
    fn backend(&self) -> &'static str {
        "switch"
    }
    async fn fixed_window_peek(&self, p: &[WindowProbe]) -> Result<Vec<u64>, CounterError> {
        self.check()?;
        self.inner.fixed_window_peek(p).await
    }
    async fn fixed_window_consume(&self, u: &[WindowUnit]) -> Result<Verdict, CounterError> {
        self.check()?;
        self.inner.fixed_window_consume(u).await
    }
    async fn bucket_take(&self, t: &[BucketTake]) -> Result<Verdict, CounterError> {
        self.check()?;
        self.inner.bucket_take(t).await
    }
    async fn health(&self) -> Result<(), CounterError> {
        self.check()
    }
}

fn limits(c: &mut aurora_config::ApiConfig) {
    c.ratelimit.api.multiplier = 1.0;
    c.ratelimit.api.classes.insert(
        "message".into(),
        aurora_config::RateClassOverride {
            per_minute: Some(6),
            burst: Some(6),
            ..Default::default()
        },
    );
}

fn breaker() -> BreakerConfig {
    BreakerConfig {
        failures: 3,
        cooldown: Duration::from_millis(300),
        timeout: Duration::from_millis(200),
    }
}

struct Fleet {
    h: Harness,
    b: Router,
    sw: Arc<Switch>,
    store: Arc<MemoryStore>,
    sess: String,
}

/// Two API replicas (own local limiter state each) on one switchable shared store.
async fn fleet(security: ClassPolicy, management: ClassPolicy) -> Fleet {
    let mut h = Harness::with_config(limits).await;
    let store = Arc::new(MemoryStore::new());
    let sw = Arc::new(Switch {
        down: AtomicBool::new(false),
        inner: store.clone(),
    });
    let mk = |sw: &Arc<Switch>| {
        ApiCounters::for_store(
            sw.clone() as Arc<dyn CounterStore>,
            security,
            management,
            breaker(),
        )
    };
    h.state = h.state.clone().with_counters(mk(&sw));
    h.app = test_router(h.state.clone());
    let b_state = AppState::new(h.db.pool().clone(), (*h.state.config).clone())
        .with_mailer(h.mail.clone())
        .with_counters(mk(&sw));
    let b = test_router(b_state);
    let org = h.org().await;
    let user = h.user("u@x.io", false).await;
    h.member(org, user, "owner", "active").await;
    let (_, sess) = h.good_session(user).await;
    Fleet {
        h,
        b,
        sw,
        store,
        sess,
    }
}

async fn send(
    app: &Router,
    m: Method,
    uri: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Value) {
    use axum::body::Body;
    use tower::ServiceExt;
    let mut b = http::Request::builder().method(m).uri(uri);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let res = app
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let (parts, body) = res.into_parts();
    let bytes = axum::body::to_bytes(body, 1 << 20).await.unwrap();
    (
        parts.status,
        parts.headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn msg() -> String {
    format!("/v1/conversations/{}/messages", Uuid::now_v7())
}

/// Alternate `n` message POSTs over the two replicas; count those that were not rate limited.
async fn message_burst(f: &Fleet, n: usize) -> (usize, Vec<StatusCode>) {
    let c = cookie(&f.sess);
    let hdr = [("cookie", c.as_str()), ("origin", ORIGIN)];
    let mut ok = 0;
    let mut all = Vec::new();
    for i in 0..n {
        let app = if i % 2 == 0 { &f.h.app } else { &f.b };
        let (s, _, _) = send(app, Method::POST, &msg(), &hdr).await;
        if s != StatusCode::TOO_MANY_REQUESTS && s != StatusCode::SERVICE_UNAVAILABLE {
            ok += 1;
        }
        all.push(s);
    }
    (ok, all)
}

#[tokio::test]
async fn two_replicas_share_one_message_limit() {
    let f = fleet(ClassPolicy::Local, ClassPolicy::Local).await;
    let (ok, all) = message_burst(&f, 20).await;
    // Burst 6 plus at most one refill token (0.1/s) while the test runs; never 2 x 6.
    assert!(
        (6..=7).contains(&ok),
        "combined allowed requests ~ the configured burst: {all:?}"
    );
}

#[tokio::test]
async fn memory_backend_still_allows_n_times_the_limit() {
    // The default: per-replica counters (today's behaviour): two replicas allow 2 x 6.
    let mut f = fleet(ClassPolicy::Local, ClassPolicy::Local).await;
    f.h.state = f.h.state.clone().with_counters(ApiCounters::local());
    f.h.app = test_router(f.h.state.clone());
    let b_state = AppState::new(f.h.db.pool().clone(), (*f.h.state.config).clone());
    f.b = test_router(b_state);
    let (ok, all) = message_burst(&f, 20).await;
    assert!((12..=13).contains(&ok), "{all:?}");
    assert!(
        f.store.keys().is_empty(),
        "the memory backend never touches a store"
    );
}

#[tokio::test]
async fn unauthenticated_per_ip_limit_is_shared() {
    let f = fleet(ClassPolicy::Local, ClassPolicy::Local).await;
    // 120/min scaled by 1.0: burst 120 shared; drive 130 requests over two replicas.
    let mut ok = 0;
    for i in 0..130 {
        let app = if i % 2 == 0 { &f.h.app } else { &f.b };
        let (s, _, _) = send(app, Method::GET, "/v1/agents", &[]).await;
        if s != StatusCode::TOO_MANY_REQUESTS {
            ok += 1;
        }
    }
    // Burst 120 (refill 2/s during the run), shared; two replicas alone would allow 240.
    assert!((120..=126).contains(&ok), "{ok}");
}

#[tokio::test]
async fn management_local_keeps_the_per_replica_limit_when_the_store_is_down() {
    let f = fleet(ClassPolicy::Local, ClassPolicy::Local).await;
    f.sw.down.store(true, Ordering::Relaxed);
    let c = cookie(&f.sess);
    let hdr = [("cookie", c.as_str()), ("origin", ORIGIN)];
    let mut ok = 0;
    for _ in 0..10 {
        let (s, _, _) = send(&f.h.app, Method::POST, &msg(), &hdr).await;
        assert_ne!(
            s,
            StatusCode::SERVICE_UNAVAILABLE,
            "local policy never answers 503"
        );
        if s != StatusCode::TOO_MANY_REQUESTS {
            ok += 1;
        }
    }
    assert!(
        (6..=7).contains(&ok),
        "down store: this replica still enforces its own limit: {ok}"
    );
    let (_, state) = f.h.state.counters.overview();
    assert_eq!(state, "fallback");
    // Recovery: after the breaker cooldown the shared store answers again.
    f.sw.down.store(false, Ordering::Relaxed);
    tokio::time::sleep(Duration::from_millis(400)).await;
    let _ = send(&f.b, Method::POST, &msg(), &hdr).await;
    assert_eq!(
        f.h.state.counters.overview().1,
        "fallback",
        "replica A has not called yet"
    );
    let _ = send(&f.h.app, Method::GET, "/v1/me", &hdr).await;
}

#[tokio::test]
async fn management_closed_answers_503_not_429() {
    let f = fleet(ClassPolicy::Local, ClassPolicy::Closed).await;
    f.sw.down.store(true, Ordering::Relaxed);
    let c = cookie(&f.sess);
    let hdr = [("cookie", c.as_str()), ("origin", ORIGIN)];
    for _ in 0..12 {
        let (s, h, body) = send(&f.h.app, Method::POST, &msg(), &hdr).await;
        assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["error"]["code"], "counter_unavailable", "{body}");
        assert_eq!(h.get("retry-after").unwrap(), "5");
    }
    // Exempt routes are never affected.
    let (s, _, _) = send(&f.h.app, Method::GET, "/healthz", &[]).await;
    assert_ne!(s, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn sign_in_limiter_is_never_unlimited_when_the_store_is_down() {
    // security = local: per-replica 120 unauthenticated GETs, then 429.
    let f = fleet(ClassPolicy::Local, ClassPolicy::Local).await;
    f.sw.down.store(true, Ordering::Relaxed);
    let mut limited = 0;
    for _ in 0..130 {
        let (s, _, _) = send(&f.h.app, Method::GET, "/v1/agents", &[]).await;
        assert_ne!(s, StatusCode::SERVICE_UNAVAILABLE);
        if s == StatusCode::TOO_MANY_REQUESTS {
            limited += 1;
        }
    }
    assert!((4..=10).contains(&limited), "never unlimited: {limited}");
    // security = closed: 503 instead.
    let f = fleet(ClassPolicy::Closed, ClassPolicy::Local).await;
    f.sw.down.store(true, Ordering::Relaxed);
    let (s, h, body) = send(&f.h.app, Method::GET, "/v1/agents", &[]).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "counter_unavailable");
    assert!(h.get("retry-after").is_some());
}

#[tokio::test]
async fn oidc_start_limit_is_shared_and_keys_hold_only_hashes() {
    let f = fleet(ClassPolicy::Local, ClassPolicy::Local).await;
    let b_state = AppState::new(f.h.db.pool().clone(), (*f.h.state.config).clone()).with_counters(
        ApiCounters::for_store(
            f.sw.clone() as Arc<dyn CounterStore>,
            ClassPolicy::Local,
            ClassPolicy::Local,
            breaker(),
        ),
    );
    let ip = "203.0.113.77";
    let mut ok = 0;
    for i in 0..30 {
        let st = if i % 2 == 0 { &f.h.state } else { &b_state };
        if aurora_api::http::counters::check_security_bucket(st, Bucket::OidcStartIp, ip)
            .await
            .is_ok()
        {
            ok += 1;
        }
    }
    assert!(
        (20..=21).contains(&ok),
        "OIDC_AUTH_RATE (20/min/IP) holds for the fleet: {ok}"
    );
    // Key privacy: no stored key contains the address, an e-mail or a token; two replicas with the
    // same signing key derive the same hash.
    let keys = f.store.keys();
    assert!(!keys.is_empty());
    for k in &keys {
        assert!(!k.contains("203.0.113"), "{k}");
        assert!(
            !k.contains('@') && !k.contains("aus_") && !k.contains("aud_"),
            "{k}"
        );
    }
    assert_eq!(
        f.h.state.derived.ratelimit_hash(ip),
        b_state.derived.ratelimit_hash(ip)
    );
    assert_ne!(
        f.h.state.derived.ratelimit_hash(ip),
        f.h.state.derived.ratelimit_hash("203.0.113.78")
    );
    assert_eq!(f.h.state.derived.ratelimit_hash(ip).len(), 32);
    // Authenticated keys hold ids only.
    let _ = message_burst(&f, 2).await;
    for k in f.store.keys() {
        assert!(k.starts_with("api:"), "{k}");
        assert!(!k.contains("u@x.io"), "{k}");
    }
}

#[tokio::test]
async fn postgres_backend_from_configuration_shares_the_limit() {
    // The real backend over the test database, built from `ratelimit.counters.*` by AppState.
    let mk = |c: &mut aurora_config::ApiConfig| {
        limits(c);
        c.counters.backend = aurora_config::CounterBackend::Postgres;
        c.counters.api_shared = true;
    };
    let h = Harness::with_config(mk).await;
    let b_state = AppState::new(h.db.pool().clone(), (*h.state.config).clone());
    let b = test_router(b_state);
    // Let the dedicated pools open their first connection (a cold handshake exceeds 50 ms).
    tokio::time::sleep(Duration::from_millis(500)).await;
    let org = h.org().await;
    let user = h.user("pg@x.io", false).await;
    h.member(org, user, "owner", "active").await;
    let (_, sess) = h.good_session(user).await;
    let c = cookie(&sess);
    let hdr = [("cookie", c.as_str()), ("origin", ORIGIN)];
    let mut ok = 0;
    for i in 0..20 {
        let app = if i % 2 == 0 { &h.app } else { &b };
        let (s, _, _) = send(app, Method::POST, &msg(), &hdr).await;
        assert_ne!(s, StatusCode::SERVICE_UNAVAILABLE);
        if s != StatusCode::TOO_MANY_REQUESTS {
            ok += 1;
        }
    }
    assert!(
        (6..=7).contains(&ok),
        "two replicas on the postgres backend share one limit: {ok}"
    );
    assert_eq!(h.state.counters.overview(), ("postgres", "shared"));
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rate_limit_buckets WHERE key LIKE 'api:%'")
            .fetch_one(h.db.pool())
            .await
            .unwrap();
    assert!(n >= 2);
    let leaked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rate_limit_buckets WHERE key LIKE '%@%' OR key LIKE '%pg@x.io%'",
    )
    .fetch_one(h.db.pool())
    .await
    .unwrap();
    assert_eq!(leaked, 0);
}
