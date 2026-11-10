//! Shared rate-limit counters (phase 4, P4-CNT, D-119, SEC-62; `docs/15 §Shared rate-limit
//! counters`).
//!
//! The [`CounterStore`] trait has three backends: [`MemoryStore`] (the default, per process, no
//! infrastructure), the Postgres one (in `aurora-db`, which implements this trait) and the
//! optional Redis one (cargo feature `redis`). [`Guarded`] wraps any store with the timeout, the
//! circuit breaker and the metrics, and [`ClassCounters`] applies the per-class unavailable
//! policy (`local` or `closed`; there is **no** fail-open value, SEC-62).
//!
//! A backend error is never read as "under the limit": callers get [`Fallback`] and must count
//! locally or refuse with 503 `counter_unavailable`.

pub mod address;
mod guard;
mod memory;
#[cfg(feature = "redis")]
pub mod redis_store;

use async_trait::async_trait;
use std::time::Duration;

pub use guard::{BreakerConfig, ClassCounters, ClassPolicy, Fallback, Guarded, State};
pub use memory::MemoryStore;

/// `COUNTER_TIMEOUT` default.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(50);
/// `COUNTER_BREAKER_FAILURES`.
pub const BREAKER_FAILURES: u32 = 3;
/// `COUNTER_BREAKER_COOLDOWN`.
pub const BREAKER_COOLDOWN: Duration = Duration::from_secs(5);
/// Longest accepted key (`03`: keys are ids and keyed hashes, at most 200 characters).
pub const MAX_KEY_LEN: usize = 200;

/// The three fixed-window periods, aligned to UTC by the store's clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Period {
    Minute,
    Hour,
    Day,
}

impl Period {
    pub const fn seconds(self) -> u64 {
        match self {
            Self::Minute => 60,
            Self::Hour => 3600,
            Self::Day => 86_400,
        }
    }
    pub const fn millis(self) -> u64 {
        self.seconds() * 1000
    }
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Minute => "minute",
            Self::Hour => "hour",
            Self::Day => "day",
        }
    }
}

/// Read the count of one `(key, period)` window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowProbe {
    pub key: String,
    pub period: Period,
}

/// Add `cost` to a window only if the result stays at most `limit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowUnit {
    pub key: String,
    pub period: Period,
    pub limit: u64,
    pub cost: u64,
}

/// One GCRA bucket: a token every `emission_interval`, holding at most `burst` tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketTake {
    pub key: String,
    pub emission_interval: Duration,
    pub burst: u32,
}

/// Result of an all-or-nothing call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allowed,
    /// Nothing was consumed. `key` is the unit/bucket that blocks the longest.
    Refused {
        key: String,
        retry_after: Duration,
    },
}

impl Verdict {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }
}

/// Store errors. Neither means "allowed".
#[derive(Debug, Clone, thiserror::Error)]
pub enum CounterError {
    /// Connection error, timeout, open breaker.
    #[error("counter store unavailable: {0}")]
    Unavailable(String),
    /// A bug (bad key, malformed answer).
    #[error("counter store invalid request: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, CounterError>;

/// Reject keys the stores must not hold.
pub fn check_key(key: &str) -> Result<()> {
    if key.is_empty() || key.len() > MAX_KEY_LEN {
        return Err(CounterError::Invalid(format!(
            "counter key must be 1..={MAX_KEY_LEN} bytes"
        )));
    }
    Ok(())
}

/// The counter backend (`15 §Shared rate-limit counters`).
#[async_trait]
pub trait CounterStore: Send + Sync {
    /// Backend name for metrics: `memory`, `postgres` or `redis`.
    fn backend(&self) -> &'static str;

    /// Current count of each window, in the order given; no side effect.
    async fn fixed_window_peek(&self, probes: &[WindowProbe]) -> Result<Vec<u64>>;

    /// Add each unit's cost to its current window iff the result stays at most its limit;
    /// all-or-nothing across the units; atomic across callers and replicas.
    async fn fixed_window_consume(&self, units: &[WindowUnit]) -> Result<Verdict>;

    /// GCRA take of one token from each bucket; all-or-nothing across the takes.
    async fn bucket_take(&self, takes: &[BucketTake]) -> Result<Verdict>;

    /// Cheap liveness probe (the breaker's half-open probe).
    async fn health(&self) -> Result<()>;
}

/// Merge units that name the same window (sum of costs, smallest limit) and sort them by key, so
/// backends can lock rows in one global order.
pub fn normalize_units(units: &[WindowUnit]) -> Vec<WindowUnit> {
    let mut out: Vec<WindowUnit> = Vec::with_capacity(units.len());
    for u in units {
        if let Some(e) = out
            .iter_mut()
            .find(|e| e.key == u.key && e.period == u.period)
        {
            e.cost = e.cost.saturating_add(u.cost);
            e.limit = e.limit.min(u.limit);
        } else {
            out.push(u.clone());
        }
    }
    out.sort_by(|a, b| (&a.key, a.period).cmp(&(&b.key, b.period)));
    out
}

/// Same for buckets: a repeated key keeps the strictest parameters; it is charged once per
/// occurrence, so occurrences are kept (callers never repeat a key in one request).
pub fn normalize_takes(takes: &[BucketTake]) -> Vec<BucketTake> {
    let mut out = takes.to_vec();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

/// Milliseconds (at least 1) of an emission interval.
pub fn interval_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX).max(1)
}

/// Retry-after of a GCRA take: how long until `max(tat, now) + T - now <= burst * T`.
/// Returns `None` when the take is allowed, else the wait in ms. Pure; shared by every backend's
/// reference arithmetic (Redis and Postgres compute the same formula in their own language).
pub fn gcra_wait_ms(tat_ms: i64, now_ms: i64, t_ms: u64, burst: u32) -> Option<u64> {
    let t = i64::try_from(t_ms).unwrap_or(i64::MAX / 4);
    let new_tat = tat_ms.max(now_ms).saturating_add(t);
    let allow_at = new_tat.saturating_sub(i64::from(burst).saturating_mul(t));
    (allow_at > now_ms).then(|| u64::try_from(allow_at - now_ms).unwrap_or(0).max(1))
}

/// The new `tat` of an allowed take.
pub fn gcra_new_tat(tat_ms: i64, now_ms: i64, t_ms: u64) -> i64 {
    tat_ms
        .max(now_ms)
        .saturating_add(i64::try_from(t_ms).unwrap_or(i64::MAX / 4))
}
