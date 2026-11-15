#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-081 (docs/05 §Shared counters, SEC-62): the SCIM plane's per-token limit follows the
//! `management` class policy when the shared counter store is down — `local` keeps enforcing
//! this replica's own limit and never answers 503, `closed` answers 503 (SCIM error format),
//! never a 429 and never "unlimited".

use crate::common::*;
use crate::scim_common::*;
use async_trait::async_trait;
use aurora_api::http::counters::ApiCounters;
use aurora_counters::{
    BreakerConfig, BucketTake, ClassPolicy, CounterError, CounterStore, MemoryStore, Verdict,
    WindowProbe, WindowUnit,
};
use http::{Method, StatusCode};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// A store that is down.
struct Down {
    down: AtomicBool,
    inner: MemoryStore,
}

impl Down {
    fn check(&self) -> Result<(), CounterError> {
        if self.down.load(Ordering::Relaxed) {
            Err(CounterError::Unavailable("stopped".into()))
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl CounterStore for Down {
    fn backend(&self) -> &'static str {
        "down"
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

/// A SCIM fixture whose API replica uses a counter store that is switched off, with the given
/// policy for the `management` class (the SCIM per-token bucket's class).
async fn fixture(management: ClassPolicy) -> ScimFx {
    let mut fx = ScimFx::new().await;
    let store = Arc::new(Down {
        down: AtomicBool::new(true),
        inner: MemoryStore::new(),
    });
    let counters = ApiCounters::for_store(
        store as Arc<dyn CounterStore>,
        ClassPolicy::Local,
        management,
        BreakerConfig {
            failures: 3,
            cooldown: Duration::from_millis(300),
            timeout: Duration::from_millis(200),
        },
    );
    fx.h.state = fx.h.state.clone().with_counters(counters);
    fx.h.app = test_router(fx.h.state.clone());
    fx
}

#[tokio::test]
async fn store_down_with_the_local_policy_keeps_this_replicas_limit() {
    let fx = fixture(ClassPolicy::Local).await;
    // SCIM_RATE 600/min, burst 120: this replica's own bucket still refuses past the burst. The
    // bucket refills 10/s, so a slow (loaded) runner needs more than burst + 10 requests to drain
    // it: keep going until the limiter answers, bounded.
    let mut statuses = Vec::new();
    for _ in 0..2000 {
        let (s, _, _) = fx.scim(Method::GET, "/scim/v2/Users?count=0", None).await;
        statuses.push(s);
        if s == StatusCode::TOO_MANY_REQUESTS {
            break;
        }
    }
    assert!(
        statuses
            .iter()
            .all(|s| *s == StatusCode::OK || *s == StatusCode::TOO_MANY_REQUESTS),
        "the local policy never answers 503: {statuses:?}"
    );
    assert!(
        statuses.contains(&StatusCode::OK),
        "the first requests are served"
    );
    assert!(
        statuses.contains(&StatusCode::TOO_MANY_REQUESTS),
        "this replica still enforces its own limit with the store down"
    );
}

/// FIX-140 item 3 (FIX-087): a `Closed` store refunds the local pre-filter's token exactly once
/// (`take_shared` refunds; `scim::rate_limit::take` must not again). The local bucket is
/// observed through how many more takes it grants: burst 3, one token spent, then one closed
/// cycle — exactly one refund leaves 2 tokens (a missing refund leaves 1, a double refund 3).
#[tokio::test]
async fn a_closed_store_refunds_the_local_token_exactly_once() {
    use aurora_api::http::apilimit::{Charge, Spec};
    let fx = fixture(ClassPolicy::Closed).await;
    let s = &fx.h.state;
    let charges = [Charge {
        key: "scim:token:fix140-refund".into(),
        // 3 per hour: the refill during the test is negligible.
        spec: Spec {
            burst: 3,
            limit: 3,
            per_secs: 3600,
        },
        scope: "org",
        class: "management",
    }];
    s.api_limiter.take(&charges).unwrap(); // 3 -> 2
    // One closed cycle exactly as `scim::rate_limit::take` runs it.
    s.api_limiter.take(&charges).unwrap(); // 2 -> 1
    let class = s.counters.management().expect("management class");
    let refused = aurora_api::http::counters::take_shared(s, class, &charges).await;
    assert!(
        matches!(refused, Err(Err(_))),
        "closed store refuses with an error"
    );
    // 2 tokens remain: two takes pass, the third is refused.
    assert!(s.api_limiter.take(&charges).is_ok());
    assert!(s.api_limiter.take(&charges).is_ok());
    assert!(
        s.api_limiter.take(&charges).is_err(),
        "a second refund would have left a third token"
    );
}

#[tokio::test]
async fn store_down_with_the_closed_policy_answers_503_in_scim_format() {
    let fx = fixture(ClassPolicy::Closed).await;
    let (s, _, v) = fx.scim(Method::GET, "/scim/v2/Users?count=0", None).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{v}");
    assert_scim_error(503, &v);
}
