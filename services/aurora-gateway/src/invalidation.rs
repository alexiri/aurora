//! Cache invalidation hub (docs/06 §Caches and invalidation, docs/05 §Event bus).
//!
//! One LISTEN connection on `aurora_gateway_invalidate` + `aurora_auth_revoked`
//! dispatches typed events to every registered cache. After a (re)connect the
//! listener yields `Resync`, on which every cache is flushed.

use crate::health::{self, Health};
use aurora_db::PgPool;
use aurora_db::bus::{BusEvent, Channel};
use sqlx::postgres::PgListener;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Org(Uuid),
    Workspace(Uuid),
    Agent(Uuid),
    Credential(Uuid),
    Connection(Uuid),
    /// `aurora_auth_revoked {kind:"user"}`: everything involving this user.
    User(Uuid),
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalidate {
    pub scope: Scope,
}

/// Size and age of a cache (`/readyz`, `/internal/v1/status`, C-380, C-381).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub entries: usize,
    /// Age of the oldest entry.
    pub oldest_age: Option<Duration>,
}

impl CacheStats {
    pub fn merge(self, o: CacheStats) -> CacheStats {
        CacheStats {
            entries: self.entries + o.entries,
            oldest_age: match (self.oldest_age, o.oldest_age) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            },
        }
    }
}

/// A gateway cache that can be told to drop entries.
pub trait Invalidatable: Send + Sync + 'static {
    fn invalidate(&self, ev: &Invalidate);
    fn flush_all(&self);
    /// Entries held and the age of the oldest (default: not a sized cache).
    fn stats(&self) -> CacheStats {
        CacheStats::default()
    }
}

/// Parse a bus notification into an event. `None` = not relevant to caches
/// (e.g. session/api_key revocations).
fn parse(channel: Channel, payload: &serde_json::Value) -> Option<Result<Invalidate, ()>> {
    let id = payload
        .get("id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());
    match channel {
        Channel::GatewayInvalidate => {
            let scope = payload.get("scope").and_then(|v| v.as_str());
            let s = match (scope, id) {
                (Some("all"), _) => Scope::All,
                (Some("org"), Some(i)) => Scope::Org(i),
                (Some("workspace"), Some(i)) => Scope::Workspace(i),
                (Some("agent"), Some(i)) => Scope::Agent(i),
                (Some("credential"), Some(i)) => Scope::Credential(i),
                (Some("connection"), Some(i)) => Scope::Connection(i),
                // Malformed: be safe and flush everything.
                _ => return Some(Err(())),
            };
            Some(Ok(Invalidate { scope: s }))
        }
        Channel::AuthRevoked => match (payload.get("kind").and_then(|v| v.as_str()), id) {
            (Some("user"), Some(i)) => Some(Ok(Invalidate {
                scope: Scope::User(i),
            })),
            _ => None,
        },
        _ => None,
    }
}

/// Resolves once the shutdown flag is true (Send-safe: no `Ref` held across await).
pub async fn stopped(rx: &mut tokio::sync::watch::Receiver<bool>) {
    let _ = rx.wait_for(|s| *s).await;
}

#[derive(Default)]
pub struct Hub {
    caches: Mutex<Vec<Arc<dyn Invalidatable>>>,
    connected: AtomicBool,
    /// Set as soon as every channel is subscribed on a (re)connected LISTEN connection, i.e.
    /// before the post-reconnect flush: anything notified from then on is received (06 (b)).
    subscribed: AtomicBool,
    health: std::sync::OnceLock<Arc<Health>>,
    /// Woken after a reconnect so the registry snapshot loader re-polls the active hash.
    registry_poke: std::sync::OnceLock<Arc<Notify>>,
    backoff: std::sync::OnceLock<(Duration, Duration)>,
    /// Approval holds waiting for a `decided`/`expired` event, by approval id.
    approval_waiters: Mutex<HashMap<Uuid, Arc<Notify>>>,
}

/// A registered wait on one approval; deregisters on drop. Wakeups carry no
/// data: the waiter re-reads the row. `notify_one` stores a permit, so an event
/// arriving before `wait` is not lost.
pub struct ApprovalWaiter {
    hub: Arc<Hub>,
    id: Uuid,
    notify: Arc<Notify>,
}

impl ApprovalWaiter {
    pub async fn wait(&self) {
        self.notify.notified().await;
    }
}

impl Drop for ApprovalWaiter {
    fn drop(&mut self) {
        if let Ok(mut w) = self.hub.approval_waiters.lock() {
            w.remove(&self.id);
        }
    }
}

impl Hub {
    /// Registers interest in events for approval `id` (register before inserting
    /// the row so nothing is missed).
    pub fn approval_waiter(self: &Arc<Self>, id: Uuid) -> ApprovalWaiter {
        let notify = Arc::new(Notify::new());
        if let Ok(mut w) = self.approval_waiters.lock() {
            w.insert(id, notify.clone());
        }
        ApprovalWaiter {
            hub: self.clone(),
            id,
            notify,
        }
    }

    fn wake_approvals(&self, only: Option<Uuid>) {
        let Ok(w) = self.approval_waiters.lock() else {
            return;
        };
        match only {
            Some(id) => {
                if let Some(n) = w.get(&id) {
                    n.notify_one();
                }
            }
            None => w.values().for_each(|n| n.notify_one()),
        }
    }

    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn register(&self, cache: Arc<dyn Invalidatable>) {
        if let Ok(mut c) = self.caches.lock() {
            c.push(cache);
        }
    }

    /// Reports the LISTEN connection to `health` (set once, before `run`).
    pub fn set_health(&self, h: Arc<Health>) {
        let _ = self.health.set(h);
    }

    /// The notifier the snapshot loader waits on (set once, before `run`).
    pub fn set_registry_poke(&self, n: Arc<Notify>) {
        let _ = self.registry_poke.set(n);
    }

    /// Overrides the reconnect backoff bounds (tests).
    pub fn set_backoff(&self, min: Duration, max: Duration) {
        let _ = self.backoff.set((min, max));
    }

    /// True while the LISTEN connection has been established.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    /// True once every channel is subscribed on the current LISTEN connection.
    pub fn is_subscribed(&self) -> bool {
        self.subscribed.load(Ordering::Acquire)
    }

    fn snapshot(&self) -> Vec<Arc<dyn Invalidatable>> {
        self.caches.lock().map(|c| c.clone()).unwrap_or_default()
    }

    /// Entries and oldest entry age over every registered cache.
    pub fn cache_stats(&self) -> CacheStats {
        self.snapshot()
            .iter()
            .fold(CacheStats::default(), |a, c| a.merge(c.stats()))
    }

    pub fn dispatch(&self, ev: &Invalidate) {
        if ev.scope == Scope::All {
            metrics::counter!("aurora_gateway_cache_flush_total", "reason" => "invalidate_all")
                .increment(1);
        }
        for c in self.snapshot() {
            c.invalidate(ev);
        }
    }

    pub fn flush_all(&self) {
        for c in self.snapshot() {
            c.flush_all();
        }
    }

    /// Flush after a LISTEN (re)connect: every notification from now on is received, those of
    /// the gap are covered by this flush.
    fn flush_after_reconnect(&self) {
        metrics::counter!("aurora_gateway_cache_flush_total", "reason" => "listener_reconnect")
            .increment(1);
        self.flush_all();
        self.wake_approvals(None);
    }

    /// Handle one bus event.
    pub fn handle(&self, ev: BusEvent) {
        match ev {
            BusEvent::Resync => self.flush_after_reconnect(),
            BusEvent::Notification {
                channel: Channel::Approval,
                payload,
            } => {
                let id = payload
                    .get("id")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Uuid::parse_str(s).ok());
                let event = payload.get("event").and_then(|v| v.as_str());
                if let (Some(id), Some("decided" | "expired")) = (id, event) {
                    self.wake_approvals(Some(id));
                }
            }
            BusEvent::Notification { channel, payload } => match parse(channel, &payload) {
                Some(Ok(ev)) => self.dispatch(&ev),
                Some(Err(())) => {
                    tracing::warn!(%payload, "malformed invalidation payload; flushing caches");
                    metrics::counter!(
                        "aurora_gateway_cache_flush_total", "reason" => "invalidate_all"
                    )
                    .increment(1);
                    self.flush_all();
                }
                None => {}
            },
        }
    }

    /// Connect (retrying) and dispatch events until `shutdown` flips to true.
    /// Caches are flushed whenever the LISTEN connection starts or restarts.
    pub async fn run(
        self: Arc<Self>,
        pool: PgPool,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        let channels = [
            Channel::GatewayInvalidate,
            Channel::AuthRevoked,
            Channel::Approval,
        ];
        let (min, max) = self
            .backoff
            .get()
            .copied()
            .unwrap_or((health::RECONNECT_BACKOFF_MIN, health::RECONNECT_BACKOFF_MAX));
        let mut first = true;
        loop {
            // (3) Connect with jittered exponential backoff; (4) the connection has re-issued
            // LISTEN on every channel when `connect_listener` returns.
            let mut backoff = min;
            let mut listener = loop {
                let attempt = tokio::select! {
                    _ = stopped(&mut shutdown) => return,
                    r = connect_listener(&pool, &channels) => r,
                };
                match attempt {
                    Ok(l) => break l,
                    Err(e) => {
                        tracing::warn!(error = %e, "invalidation listener connect failed; retrying");
                        tokio::select! {
                            _ = stopped(&mut shutdown) => return,
                            _ = tokio::time::sleep(health::jitter(backoff)) => {}
                        }
                        backoff = health::next_backoff(backoff, max);
                    }
                }
            };
            self.subscribed.store(true, Ordering::Release);
            if first {
                first = false;
            } else {
                metrics::counter!("aurora_gateway_listener_reconnects_total").increment(1);
            }
            // Invalidations sent during the gap were missed: flush *after* subscribing, so none
            // sent from here on is lost, and re-poll the active registry hash.
            self.flush_after_reconnect();
            if let Some(n) = self.registry_poke.get() {
                n.notify_one();
            }
            self.connected.store(true, Ordering::Release);
            if let Some(h) = self.health.get() {
                h.set_listener(true);
            }
            // Dispatch until the connection is lost.
            loop {
                let next = tokio::select! {
                    _ = stopped(&mut shutdown) => {
                        self.mark_listener_lost();
                        return;
                    }
                    r = listener.try_recv() => r,
                };
                match next {
                    Ok(Some(n)) => {
                        let Some(channel) = Channel::parse(n.channel()) else {
                            continue;
                        };
                        match serde_json::from_str(n.payload()) {
                            Ok(payload) => {
                                self.handle(BusEvent::Notification { channel, payload });
                            }
                            Err(e) => tracing::warn!(
                                channel = channel.as_str(),
                                error = %e,
                                "malformed notification payload skipped"
                            ),
                        }
                    }
                    // Connection lost (sqlx would re-connect inside the next call; the gateway
                    // reconnects itself so it knows when the subscription is back).
                    Ok(None) => break,
                    Err(e) => {
                        tracing::warn!(error = %e, "invalidation listener error");
                        break;
                    }
                }
            }
            drop(listener);
            // (1) Caches are kept; only the flag changes.
            self.mark_listener_lost();
            tracing::warn!("invalidation listener lost; reconnecting");
        }
    }

    fn mark_listener_lost(&self) {
        self.connected.store(false, Ordering::Release);
        self.subscribed.store(false, Ordering::Release);
        if let Some(h) = self.health.get() {
            h.set_listener(false);
        }
    }
}

/// A dedicated LISTEN connection with every channel subscribed (bounded by a 5 s timeout so a
/// stalled database cannot hold the reconnect loop).
async fn connect_listener(pool: &PgPool, channels: &[Channel]) -> Result<PgListener, String> {
    let connect = async {
        let mut l = PgListener::connect_with(pool).await?;
        l.listen_all(channels.iter().map(|c| c.as_str())).await?;
        Ok::<_, sqlx::Error>(l)
    };
    match tokio::time::timeout(Duration::from_secs(5), connect).await {
        Ok(Ok(l)) => Ok(l),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("connect timed out".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_scopes() {
        let id = Uuid::now_v7();
        let p = |c, v: serde_json::Value| parse(c, &v);
        assert_eq!(
            p(Channel::GatewayInvalidate, json!({"scope":"agent","id":id})),
            Some(Ok(Invalidate {
                scope: Scope::Agent(id)
            }))
        );
        assert_eq!(
            p(Channel::GatewayInvalidate, json!({"scope":"all"})),
            Some(Ok(Invalidate { scope: Scope::All }))
        );
        assert_eq!(
            p(Channel::GatewayInvalidate, json!({"scope":"org"})),
            Some(Err(()))
        );
        assert_eq!(
            p(Channel::AuthRevoked, json!({"kind":"user","id":id})),
            Some(Ok(Invalidate {
                scope: Scope::User(id)
            }))
        );
        assert_eq!(
            p(Channel::AuthRevoked, json!({"kind":"session","id":id})),
            None
        );
    }
}
