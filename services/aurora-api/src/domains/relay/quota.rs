//! The daily byte quota of a preview share (FIX-072, SEC-82; `docs/16 §W18` Limits):
//! `PREVIEW_BYTES_PER_SHARE_DAY` (`previews.bytes_per_share_day`, 5 GiB) per share per UTC day.
//!
//! The relays report the bytes they moved (`POST /v1/relay/report`, with every 15 s heartbeat;
//! each relay batches its streams' bytes in units of ≥ 64 KiB); the API adds them to one shared counter per share and UTC day
//! (`aurora-counters`, key `prv:bytes:<share id>`, `Period::Day`) so every relay replica sees
//! the same total. `resolve` reads it after the access checks and answers `deny` /
//! `preview_quota` once it reaches the limit; the relay renders 429 with `Retry-After` to the
//! next UTC midnight.
//!
//! **Failure policy** (SEC-62, never fail open): the `management` counter class decides, like
//! every other API limiter of comparable weight (`docs/15 §Shared rate-limit counters`):
//! `local` counts in this replica's memory until the store answers again; `closed` makes
//! `resolve` answer 503 `counter_unavailable` (the relay shows its "service unavailable"
//! page) and `report` answer 503, whereupon the relay keeps the unreported bytes and sends
//! them with the next heartbeat (the batch is one all-or-nothing consume, so nothing is
//! counted twice).
//!
//! **Overshoot bound**: a share can exceed the quota by what its relays move between two
//! heartbeats (15 s) plus the relay's resolve cache (15 s) plus the responses already open
//! (a stream that has started may finish; no new stream opens once the quota is reached).

use crate::app::AppState;
use aurora_core::ApiError;
use aurora_counters::{CounterStore, Fallback, MemoryStore, Period, WindowProbe, WindowUnit};
use std::sync::OnceLock;
use uuid::Uuid;

/// The cost ceiling of a window: the counter only adds (the limit is applied at `resolve`).
const NO_LIMIT: u64 = 1 << 60;

/// This replica's own counts, used under the `local` class policy and when the backend is
/// `memory` (no shared class exists).
fn local() -> &'static MemoryStore {
    static LOCAL: OnceLock<MemoryStore> = OnceLock::new();
    LOCAL.get_or_init(MemoryStore::new)
}

fn key(share: Uuid) -> String {
    format!("prv:bytes:{share}")
}

fn unavailable() -> ApiError {
    ApiError::counter_unavailable("preview_quota")
}

/// Bytes relayed for `share` so far in the current UTC day.
pub async fn used_today(s: &AppState, share: Uuid) -> Result<u64, ApiError> {
    let probes = [WindowProbe {
        key: key(share),
        period: Period::Day,
    }];
    let counts = match s.counters.management() {
        None => local().fixed_window_peek(&probes).await,
        Some(class) => match class.peek(&probes).await {
            Ok(v) => Ok(v),
            Err(Fallback::Local) => local().fixed_window_peek(&probes).await,
            Err(Fallback::Closed) => return Err(unavailable()),
        },
    };
    counts
        .map(|v| v.first().copied().unwrap_or(0))
        .map_err(|_| unavailable())
}

/// Whether `share` has reached its daily quota.
pub async fn exceeded(s: &AppState, share: Uuid) -> Result<bool, ApiError> {
    Ok(used_today(s, share).await? >= s.config.previews.bytes_per_share_day)
}

/// Adds the relayed bytes (`(share, bytes)`, both directions summed) to the shares' daily
/// counters in one all-or-nothing call.
pub async fn add(s: &AppState, per_share: &[(Uuid, u64)]) -> Result<(), ApiError> {
    let units: Vec<WindowUnit> = per_share
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(id, n)| WindowUnit {
            key: key(*id),
            period: Period::Day,
            limit: NO_LIMIT,
            cost: *n,
        })
        .collect();
    if units.is_empty() {
        return Ok(());
    }
    let r = match s.counters.management() {
        None => local().fixed_window_consume(&units).await,
        Some(class) => match class.consume(&units).await {
            Ok(v) => Ok(v),
            Err(Fallback::Local) => local().fixed_window_consume(&units).await,
            Err(Fallback::Closed) => return Err(unavailable()),
        },
    };
    r.map(|_| ()).map_err(|_| unavailable())
}
