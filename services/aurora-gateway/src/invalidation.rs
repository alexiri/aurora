//! Cache invalidation hub (docs/06 §Caches and invalidation, docs/05 §Event bus).
//!
//! One LISTEN connection on `aurora_gateway_invalidate` + `aurora_auth_revoked`
//! dispatches typed events to every registered cache. After a (re)connect the
//! listener yields `Resync`, on which every cache is flushed.

use aurora_db::PgPool;
use aurora_db::bus::{BusEvent, Channel, Listener};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
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

/// A gateway cache that can be told to drop entries.
pub trait Invalidatable: Send + Sync + 'static {
    fn invalidate(&self, ev: &Invalidate);
    fn flush_all(&self);
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
}

impl Hub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn register(&self, cache: Arc<dyn Invalidatable>) {
        if let Ok(mut c) = self.caches.lock() {
            c.push(cache);
        }
    }

    /// True while the LISTEN connection has been established.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    fn snapshot(&self) -> Vec<Arc<dyn Invalidatable>> {
        self.caches.lock().map(|c| c.clone()).unwrap_or_default()
    }

    pub fn dispatch(&self, ev: &Invalidate) {
        for c in self.snapshot() {
            c.invalidate(ev);
        }
    }

    pub fn flush_all(&self) {
        for c in self.snapshot() {
            c.flush_all();
        }
    }

    /// Handle one bus event.
    pub fn handle(&self, ev: BusEvent) {
        match ev {
            BusEvent::Resync => self.flush_all(),
            BusEvent::Notification { channel, payload } => match parse(channel, &payload) {
                Some(Ok(ev)) => self.dispatch(&ev),
                Some(Err(())) => {
                    tracing::warn!(%payload, "malformed invalidation payload; flushing caches");
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
        let channels = [Channel::GatewayInvalidate, Channel::AuthRevoked];
        let mut backoff = Duration::from_millis(250);
        let mut listener = loop {
            let attempt = tokio::select! {
                _ = stopped(&mut shutdown) => return,
                r = Listener::connect(&pool, &channels) => r,
            };
            match attempt {
                Ok(l) => break l,
                Err(e) => {
                    tracing::warn!(error = %e, "invalidation listener connect failed; retrying");
                    tokio::select! {
                        _ = stopped(&mut shutdown) => return,
                        _ = tokio::time::sleep(backoff) => {}
                    }
                    backoff = (backoff * 2).min(Duration::from_secs(10));
                }
            }
        };
        self.connected.store(true, Ordering::Release);
        loop {
            tokio::select! {
                _ = stopped(&mut shutdown) => break,
                ev = listener.next() => self.handle(ev),
            }
        }
        self.connected.store(false, Ordering::Release);
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
