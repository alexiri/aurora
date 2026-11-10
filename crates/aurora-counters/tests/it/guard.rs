//! Breaker, timeout and class policy (SEC-62): a store failure is never "allowed".

use async_trait::async_trait;
use aurora_counters::{
    BreakerConfig, BucketTake, ClassCounters, ClassPolicy, CounterError, CounterStore, Fallback,
    Guarded, MemoryStore, Period, State, Verdict, WindowProbe, WindowUnit,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::time::Duration;

/// 0 = healthy, 1 = error, 2 = hang.
struct Fault {
    mode: AtomicU8,
    calls: AtomicU32,
    inner: MemoryStore,
}

impl Fault {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            mode: AtomicU8::new(0),
            calls: AtomicU32::new(0),
            inner: MemoryStore::new(),
        })
    }
    async fn gate(&self) -> Result<(), CounterError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        match self.mode.load(Ordering::Relaxed) {
            0 => Ok(()),
            1 => Err(CounterError::Unavailable("injected".into())),
            2 => {
                tokio::time::sleep(Duration::from_secs(30)).await;
                Ok(())
            }
            _ => Err(CounterError::Invalid("injected bug".into())),
        }
    }
}

#[async_trait]
impl CounterStore for Fault {
    fn backend(&self) -> &'static str {
        "fault"
    }
    async fn fixed_window_peek(&self, p: &[WindowProbe]) -> Result<Vec<u64>, CounterError> {
        self.gate().await?;
        self.inner.fixed_window_peek(p).await
    }
    async fn fixed_window_consume(&self, u: &[WindowUnit]) -> Result<Verdict, CounterError> {
        self.gate().await?;
        self.inner.fixed_window_consume(u).await
    }
    async fn bucket_take(&self, t: &[BucketTake]) -> Result<Verdict, CounterError> {
        self.gate().await?;
        self.inner.bucket_take(t).await
    }
    async fn health(&self) -> Result<(), CounterError> {
        self.gate().await
    }
}

fn unit() -> WindowUnit {
    WindowUnit {
        key: "k".into(),
        period: Period::Hour,
        limit: 100,
        cost: 1,
    }
}

fn cfg() -> BreakerConfig {
    BreakerConfig {
        failures: 3,
        cooldown: Duration::from_millis(200),
        timeout: Duration::from_millis(50),
    }
}

#[tokio::test]
async fn breaker_opens_after_three_failures_and_probes_after_the_cooldown() {
    let f = Fault::new();
    let g = Guarded::new(f.clone(), cfg());
    assert!(
        g.fixed_window_consume(&[unit()])
            .await
            .expect("healthy")
            .is_allowed()
    );
    f.mode.store(1, Ordering::Relaxed);
    f.calls.store(0, Ordering::Relaxed);
    for _ in 0..3 {
        assert!(matches!(
            g.fixed_window_consume(&[unit()]).await,
            Err(CounterError::Unavailable(_))
        ));
    }
    assert!(g.is_open());
    // Open: answered without calling the store.
    let before = f.calls.load(Ordering::Relaxed);
    for _ in 0..5 {
        assert!(g.fixed_window_consume(&[unit()]).await.is_err());
    }
    assert_eq!(
        f.calls.load(Ordering::Relaxed),
        before,
        "an open breaker never calls the store"
    );
    // After the cooldown one probe decides; a failing probe keeps it open.
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(g.fixed_window_consume(&[unit()]).await.is_err());
    assert!(g.is_open());
    // The store recovers: the next probe closes it.
    f.mode.store(0, Ordering::Relaxed);
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        g.fixed_window_consume(&[unit()])
            .await
            .expect("recovered")
            .is_allowed()
    );
    assert!(!g.is_open());
}

#[tokio::test]
async fn a_hang_is_a_timeout_and_never_allowed() {
    let f = Fault::new();
    f.mode.store(2, Ordering::Relaxed);
    let g = Guarded::new(f.clone(), cfg());
    let started = std::time::Instant::now();
    let r = g.fixed_window_consume(&[unit()]).await;
    assert!(matches!(r, Err(CounterError::Unavailable(_))));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn invalid_is_never_read_as_allowed() {
    let f = Fault::new();
    f.mode.store(3, Ordering::Relaxed);
    let g = Guarded::new(f, cfg());
    assert!(matches!(
        g.bucket_take(&[]).await,
        Err(CounterError::Unavailable(_))
    ));
}

#[tokio::test]
async fn class_policy_local_and_closed() {
    let f = Fault::new();
    let g: Arc<dyn CounterStore> = Arc::new(Guarded::new(f.clone(), cfg()));
    let local = ClassCounters::new(g.clone(), "management", ClassPolicy::Local);
    let closed = ClassCounters::new(g.clone(), "security", ClassPolicy::Closed);
    assert!(local.consume(&[unit()]).await.expect("shared").is_allowed());
    assert_eq!(local.state(), State::Shared);
    f.mode.store(1, Ordering::Relaxed);
    assert_eq!(local.consume(&[unit()]).await.unwrap_err(), Fallback::Local);
    assert_eq!(
        closed.consume(&[unit()]).await.unwrap_err(),
        Fallback::Closed
    );
    assert_eq!(local.state(), State::Fallback);
    assert_eq!(closed.peek(&[]).await.unwrap_err(), Fallback::Closed);
    f.mode.store(0, Ordering::Relaxed);
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        local
            .consume(&[unit()])
            .await
            .expect("recovered")
            .is_allowed()
    );
    assert_eq!(local.state(), State::Shared);
}

#[test]
fn there_is_no_open_policy() {
    assert_eq!(ClassPolicy::parse("local"), Ok(ClassPolicy::Local));
    assert_eq!(ClassPolicy::parse("closed"), Ok(ClassPolicy::Closed));
    for bad in ["open", "OPEN", "Local", "", "fail_open", "allow"] {
        let e = ClassPolicy::parse(bad).unwrap_err();
        assert!(e.contains("fail-open"), "{e}");
    }
}
