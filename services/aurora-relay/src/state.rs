//! Shared relay state: the API client, the registration heartbeat and the resolve cache
//! (`docs/15 §Port previews`: the relay keeps no state beyond in-memory caches; a restart
//! loses only open streams).
//!
//! Resolve answers are cached at most `PREVIEW_AUTH_CACHE` (15 s; 5 s for a negative answer)
//! keyed by `(label, session_hash)`. Open streams are re-validated every cycle through
//! `POST /v1/relay/validate` and closed with 4403 when no longer allowed (worst-case latency
//! 15 s + one round trip, `docs/04` step 5).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::api::RelayApi;

/// One cached resolve answer.
#[derive(Clone)]
pub struct CachedResolve {
    pub decision: String,
    /// Uniform `not_found` or the specific spoke reason.
    pub reason: Option<String>,
    /// `true` = positive entry (15 s), `false` = negative (5 s).
    pub positive: bool,
    pub at: Instant,
    /// What the stream caps need from an `allow` (kept so a cache hit is as complete as the
    /// API's answer): the share and the session's user.
    pub share: Option<crate::api::ShareRef>,
    pub user: Option<uuid::Uuid>,
}

impl CachedResolve {
    fn fresh(&self, pos: Duration, neg: Duration) -> bool {
        let ttl = if self.positive { pos } else { neg };
        self.at.elapsed() < ttl
    }
}

pub struct State {
    pub api: Arc<RelayApi>,
    /// The API's `previews.domain` (from the register answer); the relay refuses to serve
    /// when its own differs, so after start these are equal.
    pub domain: String,
    /// The dashboard origin for the authorize redirect.
    pub external_url: String,
    pub wake_wait_secs: u32,
    /// `PREVIEW_GRANT_RATE` at the grant endpoint (per replica).
    pub grant_rate: crate::limits::GrantRate,
    cache: Mutex<HashMap<(String, String), CachedResolve>>,
}

const CACHE_POS: Duration = Duration::from_secs(15);
const CACHE_NEG: Duration = Duration::from_secs(5);
/// Cache entries beyond this count are dropped wholesale (labels are bounded anyway).
const CACHE_MAX: usize = 100_000;

impl State {
    pub fn new(
        api: Arc<RelayApi>,
        domain: String,
        external_url: String,
        wake_wait_secs: u32,
    ) -> Arc<Self> {
        Self::with_grant_rate(
            api,
            domain,
            external_url,
            wake_wait_secs,
            crate::limits::GrantRate::spec(),
        )
    }

    /// `new` with explicit grant-redemption limits (tests).
    pub fn with_grant_rate(
        api: Arc<RelayApi>,
        domain: String,
        external_url: String,
        wake_wait_secs: u32,
        grant_rate: crate::limits::GrantRate,
    ) -> Arc<Self> {
        Arc::new(Self {
            api,
            domain,
            external_url,
            wake_wait_secs,
            grant_rate,
            cache: Mutex::new(HashMap::new()),
        })
    }

    /// The cached answer for `(label, hash)`, if any is still fresh.
    pub async fn cached(&self, label: &str, hash: &str) -> Option<CachedResolve> {
        let g = self.cache.lock().await;
        g.get(&(label.to_owned(), hash.to_owned()))
            .filter(|c| c.fresh(CACHE_POS, CACHE_NEG))
            .cloned()
    }

    /// Stores an answer (positive when the decision is `allow`).
    pub async fn cache_put(&self, label: &str, hash: &str, a: &crate::api::ResolveAnswer) {
        let mut g = self.cache.lock().await;
        if g.len() >= CACHE_MAX {
            g.clear();
        }
        g.insert(
            (label.to_owned(), hash.to_owned()),
            CachedResolve {
                decision: a.decision.clone(),
                reason: a.reason.clone(),
                positive: a.decision == "allow",
                at: Instant::now(),
                share: a.share.clone(),
                user: a.user,
            },
        );
    }

    /// The resolve of one request: cache first, then the API (long-poll only when the relay
    /// itself has to wait for a wake; the edge-facing path never waits).
    pub async fn resolve(
        &self,
        label: &str,
        hash: Option<&str>,
        navigation: bool,
    ) -> Result<crate::api::ResolveAnswer, crate::api::ApiError> {
        let key = hash.unwrap_or("");
        if let Some(c) = self.cached(label, key).await {
            metrics::counter!("aurora_relay_resolve_cache", "hit" => "true").increment(1);
            return Ok(crate::api::ResolveAnswer {
                decision: c.decision,
                reason: c.reason,
                share: c.share,
                route: None,
                user: c.user,
                retry_after_secs: None,
            });
        }
        metrics::counter!("aurora_relay_resolve_cache", "hit" => "false").increment(1);
        let a = self.api.resolve(label, hash, 0, navigation).await?;
        // A `wake_limited` answer is not cached: its `retry_after_secs` is the API's, and the
        // budget frees a slot on the API's clock, not after a cache entry.
        if a.reason.as_deref() != Some("wake_limited") {
            self.cache_put(label, key, &a).await;
        }
        Ok(a)
    }
}

/// A live `(label, session_hash)` pair of open streams.
pub type LivePair = (String, String);

/// Puts back the report entries whose delivery failed.
pub type RestoreFn = Arc<dyn Fn(&[crate::api::ReportShare]) + Send + Sync>;

/// Validation callbacks: the pairs to check and what to do with the revoked ones.
pub struct ValidateHooks {
    pub live: Arc<dyn Fn() -> Vec<LivePair> + Send + Sync>,
    pub on_revoked: Arc<dyn Fn(Vec<LivePair>) + Send + Sync>,
    /// The bytes relayed per share since the last heartbeat (the API counts them against the
    /// daily quota); drained each cycle.
    pub report: Arc<dyn Fn() -> Vec<crate::api::ReportShare> + Send + Sync>,
    /// Puts back a report whose delivery failed.
    pub restore: RestoreFn,
}

/// Registration heartbeat: registers once, then keeps the replica alive with `report`
/// every 15 s; re-registers when the token stops being accepted (rotation).
pub async fn registration_loop(
    api: Arc<RelayApi>,
    version: &'static str,
    mut stop: tokio::sync::watch::Receiver<bool>,
) -> Option<(String, String, u32)> {
    loop {
        match api.register(version).await {
            Ok(r) => {
                if r.protocol != 1 {
                    tracing::error!(
                        protocol = r.protocol,
                        "API speaks an unsupported relay protocol"
                    );
                    return None;
                }
                tracing::info!(replica = %r.replica_id, "registered with the API");
                return Some((
                    r.config.previews_domain,
                    r.config.external_url,
                    r.config.wake_wait_secs,
                ));
            }
            Err(e) => {
                tracing::warn!(error = %e, "register failed; retrying");
                tokio::select! {
                    () = tokio::time::sleep(Duration::from_secs(3)) => {}
                    _ = stop.changed() => return None,
                }
            }
        }
    }
}

/// The 15 s heartbeat + validation cycle. `hooks.live` reports the currently open
/// `(label, session_hash)` pairs of open streams; revoked pairs go to `hooks.on_revoked`
/// so the stream table can close them with 4403.
pub async fn heartbeat_loop(
    api: Arc<RelayApi>,
    hooks: ValidateHooks,
    every: Duration,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    let mut tick = tokio::time::interval(every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = stop.changed() => return,
        }
        // Re-validate open streams first (docs/04 step 5): revoked → close 4403.
        let pairs = (hooks.live)();
        if !pairs.is_empty() {
            match api.validate(&pairs).await {
                Ok(revoked) => {
                    if !revoked.is_empty() {
                        tracing::info!(revoked = revoked.len(), "preview sessions revoked");
                        (hooks.on_revoked)(revoked);
                    }
                }
                Err(e) => tracing::warn!(error = %e, "validate failed"),
            }
        }
        // The heartbeat itself (an empty list is still a liveness report) carries the bytes
        // relayed per share since the last one: the API adds them to the share's daily
        // counter (`PREVIEW_BYTES_PER_SHARE_DAY`). A failed report is retried with the next.
        let shares = (hooks.report)();
        if let Err(e) = api.report(&shares).await {
            tracing::warn!(error = %e, "relay report failed");
            (hooks.restore)(&shares);
        }
    }
}
