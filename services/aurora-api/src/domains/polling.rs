//! Shared helpers of the git-source poll jobs (addon sources C-598, template sources
//! C-1107): one implementation of the owner's interval enum, the per-row jitter, the lease
//! owner id and the poll-loop skeleton. Both domains behave identically to when they each
//! held a copy (P6-TPL-2 extracted them; the addon tests stayed untouched).

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use uuid::Uuid;

use aurora_core::{ApiError, ids};

/// The interval values of the owner's enum.
pub const INTERVALS: [&str; 7] = ["6h", "12h", "1d", "1w", "2w", "1mo", "never"];

/// Whether `v` is one of [`INTERVALS`].
#[must_use]
pub fn valid_interval(v: &str) -> bool {
    INTERVALS.contains(&v)
}

/// Interval seconds for the owner's enum (`None` for `never`).
#[must_use]
pub fn interval_secs(interval: &str) -> Option<i64> {
    match interval {
        "6h" => Some(6 * 3600),
        "12h" => Some(12 * 3600),
        "1d" => Some(24 * 3600),
        "1w" => Some(7 * 24 * 3600),
        "2w" => Some(14 * 24 * 3600),
        "1mo" => Some(30 * 24 * 3600),
        _ => None,
    }
}

/// ±10 % jitter keyed by the source id (stable per row, no clock read).
#[must_use]
pub fn jitter(id: Uuid) -> f64 {
    let n = (id.as_u128() % 2001) as f64 / 10_000.0 - 0.1;
    1.0 + n
}

/// Seconds until the next scheduled check, `None` for `never` (no schedule: manual only).
#[must_use]
pub fn next_in_secs(interval: &str, id: Uuid) -> Option<i64> {
    interval_secs(interval).map(|s| ((s as f64) * jitter(id)) as i64)
}

/// The per-process lease owner id (random per call, like the library refresh).
static OWNER: AtomicU64 = AtomicU64::new(0);

#[must_use]
pub fn lease_owner() -> Uuid {
    let mut b = *ids::new_id().as_bytes();
    b[0] = OWNER.fetch_add(1, Ordering::Relaxed) as u8;
    Uuid::from_bytes(b)
}

/// The tick of both poll loops: one sweep a minute; `claim_due` does the scheduling.
const TICK: Duration = Duration::from_secs(60);

/// Starts a poll loop: every [`TICK`], call `once`; a failed sweep only logs (the next tick
/// retries). Safe on every replica at once — the claim query holds the lease.
pub fn spawn_loop<F, Fut>(job: &'static str, state: crate::app::AppState, once: F)
where
    F: Fn(crate::app::AppState) -> Fut + Send + 'static,
    Fut: Future<Output = Result<usize, ApiError>> + Send,
{
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if let Err(e) = once(state.clone()).await {
                tracing::warn!(error = %e.message(), "{job} failed");
            }
        }
    });
}
