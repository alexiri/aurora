//! Timeout, circuit breaker, metrics ([`Guarded`]) and the per-class unavailable policy
//! ([`ClassCounters`]), implemented once for every backend and limiter (`15 §Shared rate-limit
//! counters`, SEC-62).

use crate::{
    BREAKER_COOLDOWN, BREAKER_FAILURES, BucketTake, CounterError, CounterStore, DEFAULT_TIMEOUT,
    Result, Verdict, WindowProbe, WindowUnit,
};
use async_trait::async_trait;
use std::future::Future;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub struct BreakerConfig {
    pub failures: u32,
    pub cooldown: Duration,
    pub timeout: Duration,
}

impl Default for BreakerConfig {
    fn default() -> Self {
        Self {
            failures: BREAKER_FAILURES,
            cooldown: BREAKER_COOLDOWN,
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

#[derive(Default)]
struct Breaker {
    failures: u32,
    open_until: Option<Instant>,
    probing: bool,
}

/// What the breaker says before a call.
enum Gate {
    Pass,
    /// Cooldown elapsed: this caller runs the liveness probe.
    Probe,
    Open,
}

/// A store wrapped with `COUNTER_TIMEOUT`, the breaker and the `aurora_counters_*` metrics.
/// Itself a [`CounterStore`]: every error it returns is [`CounterError::Unavailable`].
pub struct Guarded {
    inner: Arc<dyn CounterStore>,
    core: Arc<Core>,
}

/// The breaker's state, shared with the probe task it owns (FIX-087: the probe's answer, not
/// the cancellable caller, decides the breaker).
struct Core {
    cfg: BreakerConfig,
    breaker: Mutex<Breaker>,
}

impl Guarded {
    pub fn new(inner: Arc<dyn CounterStore>, cfg: BreakerConfig) -> Self {
        Self {
            inner,
            core: Arc::new(Core {
                cfg,
                breaker: Mutex::new(Breaker::default()),
            }),
        }
    }

    pub fn inner_backend(&self) -> &'static str {
        self.inner.backend()
    }

    fn gate(&self) -> Gate {
        self.core.gate()
    }

    fn success(&self) {
        self.core.success();
    }

    fn failure(&self) {
        self.core.failure();
    }

    /// Whether the breaker is currently open (tests, metrics).
    pub fn is_open(&self) -> bool {
        self.core
            .breaker
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .open_until
            .is_some()
    }

    /// Run the liveness probe in a task the breaker owns: a caller that is dropped while
    /// waiting neither records a failure nor re-arms the cooldown; the probe's own answer
    /// closes or re-arms the breaker. A panic in the probe counts as a failure.
    async fn probe(&self) -> bool {
        let core = self.core.clone();
        let inner = self.inner.clone();
        let task = tokio::spawn(async move {
            let mut pending = PendingProbe {
                core: core.clone(),
                decided: false,
            };
            let ok = matches!(
                tokio::time::timeout(core.cfg.timeout, inner.health()).await,
                Ok(Ok(()))
            );
            pending.decided = true;
            if ok {
                core.success();
            } else {
                core.failure();
            }
            ok
        });
        task.await.unwrap_or(false)
    }
}

/// Dropped undecided (the probe task panicked or the runtime is shutting down): record a
/// failure so `probing` is cleared and the breaker can never wedge open.
struct PendingProbe {
    core: Arc<Core>,
    decided: bool,
}

impl Drop for PendingProbe {
    fn drop(&mut self) {
        if !self.decided {
            self.core.failure();
        }
    }
}

impl Core {
    fn gate(&self) -> Gate {
        let mut b = self.breaker.lock().unwrap_or_else(|p| p.into_inner());
        match b.open_until {
            None => Gate::Pass,
            Some(t) if Instant::now() < t => Gate::Open,
            Some(_) if b.probing => Gate::Open,
            Some(_) => {
                b.probing = true;
                Gate::Probe
            }
        }
    }

    fn success(&self) {
        let mut b = self.breaker.lock().unwrap_or_else(|p| p.into_inner());
        *b = Breaker::default();
    }

    fn failure(&self) {
        let mut b = self.breaker.lock().unwrap_or_else(|p| p.into_inner());
        b.probing = false;
        b.failures = b.failures.saturating_add(1);
        if b.failures >= self.cfg.failures || b.open_until.is_some() {
            b.open_until = Some(Instant::now() + self.cfg.cooldown);
        }
    }
}

impl Guarded {
    async fn run<T, F>(&self, op: &'static str, f: F) -> Result<T>
    where
        F: Future<Output = Result<T>>,
    {
        let backend = self.inner.backend();
        let started = Instant::now();
        match self.gate() {
            Gate::Open => {
                count(backend, op, "breaker_open");
                return Err(CounterError::Unavailable("circuit breaker open".into()));
            }
            Gate::Probe => {
                // The probe runs in a task the breaker owns; if this caller is dropped
                // meanwhile (client disconnect, freeze) the probe still decides the breaker.
                if !self.probe().await {
                    count(backend, op, "breaker_open");
                    return Err(CounterError::Unavailable("circuit breaker open".into()));
                }
            }
            Gate::Pass => {}
        }
        let r = tokio::time::timeout(self.core.cfg.timeout, f).await;
        metrics::histogram!(
            "aurora_counters_call_seconds",
            "backend" => backend,
            "op" => op
        )
        .record(started.elapsed().as_secs_f64());
        match r {
            Ok(Ok(v)) => {
                self.success();
                Ok(v)
            }
            Ok(Err(e)) => {
                self.failure();
                count(backend, op, "error");
                tracing::debug!(error = %e, backend, op, "counter store call failed");
                // `Invalid` is a bug, never "allowed": callers treat it as unavailable.
                Err(CounterError::Unavailable(e.to_string()))
            }
            Err(_) => {
                self.failure();
                count(backend, op, "timeout");
                Err(CounterError::Unavailable("timeout".into()))
            }
        }
    }
}

fn count(backend: &'static str, op: &'static str, result: &'static str) {
    metrics::counter!(
        "aurora_counters_calls_total",
        "backend" => backend,
        "op" => op,
        "result" => result
    )
    .increment(1);
}

fn verdict_label(v: &Verdict) -> &'static str {
    if v.is_allowed() { "allowed" } else { "refused" }
}

#[async_trait]
impl CounterStore for Guarded {
    fn backend(&self) -> &'static str {
        self.inner.backend()
    }

    async fn fixed_window_peek(&self, probes: &[WindowProbe]) -> Result<Vec<u64>> {
        let r = self.run("peek", self.inner.fixed_window_peek(probes)).await;
        if r.is_ok() {
            count(self.inner.backend(), "peek", "allowed");
        }
        r
    }

    async fn fixed_window_consume(&self, units: &[WindowUnit]) -> Result<Verdict> {
        let r = self
            .run("consume", self.inner.fixed_window_consume(units))
            .await;
        if let Ok(v) = &r {
            count(self.inner.backend(), "consume", verdict_label(v));
        }
        r
    }

    async fn bucket_take(&self, takes: &[BucketTake]) -> Result<Verdict> {
        let r = self.run("take", self.inner.bucket_take(takes)).await;
        if let Ok(v) = &r {
            count(self.inner.backend(), "take", verdict_label(v));
        }
        r
    }

    async fn health(&self) -> Result<()> {
        self.run("health", self.inner.health()).await
    }
}

/// What the unavailable-backend policy of a class says (`ratelimit.counters.on_unavailable.*`).
/// There is no `open`: [`ClassPolicy::parse`] refuses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassPolicy {
    Local,
    Closed,
}

impl ClassPolicy {
    /// `local` or `closed`; anything else (`open` included) is an error naming the value.
    pub fn parse(v: &str) -> std::result::Result<Self, String> {
        match v {
            "local" => Ok(Self::Local),
            "closed" => Ok(Self::Closed),
            other => Err(format!(
                "`{other}` is not allowed: use `local` or `closed` (there is no fail-open value)"
            )),
        }
    }
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Closed => "closed",
        }
    }
}

/// The caller's instruction when the shared store could not answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback {
    /// Count in this replica's own memory until the store answers again.
    Local,
    /// Refuse with 503 `counter_unavailable`.
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Shared,
    Fallback,
}

impl State {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Fallback => "fallback",
        }
    }
}

/// One limiter class (`security`, `management`, `gateway`) over the shared store.
pub struct ClassCounters {
    store: Arc<dyn CounterStore>,
    class: &'static str,
    policy: ClassPolicy,
    /// 0 = unknown (never called), 1 = shared, 2 = fallback.
    state: AtomicU8,
}

impl ClassCounters {
    pub fn new(store: Arc<dyn CounterStore>, class: &'static str, policy: ClassPolicy) -> Self {
        let c = Self {
            store,
            class,
            policy,
            state: AtomicU8::new(0),
        };
        c.publish(State::Shared);
        c.state.store(0, Ordering::Relaxed);
        c
    }

    pub fn class(&self) -> &'static str {
        self.class
    }

    pub fn policy(&self) -> ClassPolicy {
        self.policy
    }

    pub fn backend(&self) -> &'static str {
        self.store.backend()
    }

    /// Current state (`Shared` until a call has failed).
    pub fn state(&self) -> State {
        if self.state.load(Ordering::Relaxed) == 2 {
            State::Fallback
        } else {
            State::Shared
        }
    }

    fn publish(&self, s: State) {
        let b = self.store.backend();
        for st in [State::Shared, State::Fallback] {
            metrics::gauge!(
                "aurora_counters_state",
                "backend" => b,
                "class" => self.class,
                "state" => st.as_str()
            )
            .set(if st == s { 1.0 } else { 0.0 });
        }
    }

    fn ok(&self) {
        if self.state.swap(1, Ordering::Relaxed) == 2 {
            tracing::info!(class = self.class, "shared counters recovered");
            self.publish(State::Shared);
        }
    }

    fn unavailable(&self, err: &CounterError) -> Fallback {
        if self.state.swap(2, Ordering::Relaxed) != 2 {
            tracing::warn!(
                class = self.class,
                policy = self.policy.as_str(),
                error = %err,
                "shared counters unavailable; applying the class policy"
            );
            self.publish(State::Fallback);
        }
        metrics::counter!(
            "aurora_counters_fallback_total",
            "class" => self.class,
            "policy" => self.policy.as_str()
        )
        .increment(1);
        match self.policy {
            ClassPolicy::Local => Fallback::Local,
            ClassPolicy::Closed => Fallback::Closed,
        }
    }

    fn map<T>(&self, r: Result<T>) -> std::result::Result<T, Fallback> {
        match r {
            Ok(v) => {
                self.ok();
                Ok(v)
            }
            Err(e) => Err(self.unavailable(&e)),
        }
    }

    pub async fn peek(&self, probes: &[WindowProbe]) -> std::result::Result<Vec<u64>, Fallback> {
        self.map(self.store.fixed_window_peek(probes).await)
    }

    pub async fn consume(&self, units: &[WindowUnit]) -> std::result::Result<Verdict, Fallback> {
        self.map(self.store.fixed_window_consume(units).await)
    }

    pub async fn take(&self, takes: &[BucketTake]) -> std::result::Result<Verdict, Fallback> {
        self.map(self.store.bucket_take(takes).await)
    }
}
