//! Registry snapshot loading (docs/06 §Registry snapshot loading, docs/09 §9).
//!
//! The active snapshot (highest supported contract, then most recent activation) is
//! parsed, validated, and swapped atomically. A snapshot that names anything this
//! gateway does not understand is refused and the previous one kept.

use arc_swap::ArcSwapOption;
use aurora_db::PgPool;
use aurora_db::bus::{BusEvent, Channel, Listener};
use aurora_policy::HostPattern;
use aurora_registry::index::{HostIndex, Snapshot};
use aurora_registry::manifest::{INSTANCE_HOST, Inject};
use aurora_registry::snapshot::{ClaimStatus, HostEntry, select_active};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Highest manifest/snapshot contract this gateway implements.
pub const SUPPORTED_CONTRACT: u32 = 1;
pub const POLL_INTERVAL: Duration = Duration::from_secs(30);

/// A validated snapshot with its content hash.
#[derive(Debug)]
pub struct Loaded {
    pub hash: Arc<str>,
    pub snapshot: Snapshot,
}

impl Loaded {
    pub fn new(hash: &str, snapshot: Snapshot) -> Result<Self, String> {
        validate(&snapshot)?;
        Ok(Self {
            hash: Arc::from(hash),
            snapshot,
        })
    }

    /// The winning (non-lost) host entry for the request.
    pub fn lookup(&self, host: &str, port: u16, path: &str) -> Option<&HostEntry> {
        HostIndex::new(&self.snapshot).lookup_port(host, port, path)
    }

    /// Active host entries whose host pattern matches, whatever the path.
    pub fn entries_for_host(&self, host: &str, port: u16) -> Vec<&HostEntry> {
        self.snapshot
            .hosts
            .iter()
            .filter(|h| h.claim_status == ClaimStatus::Active)
            .filter(|h| HostPattern::lenient(&h.host).matches(host, port))
            .collect()
    }

    /// Governed LLM hosts are never app hosts (SEC-37).
    pub fn is_reserved(&self, host: &str, port: u16) -> bool {
        self.snapshot
            .reserved_hosts
            .iter()
            .any(|p| HostPattern::lenient(p).matches(host, port))
    }
}

/// Everything beyond deserialization that makes a snapshot unusable here.
fn validate(s: &Snapshot) -> Result<(), String> {
    if s.contract > SUPPORTED_CONTRACT {
        return Err(format!("unsupported contract {}", s.contract));
    }
    // (P4-GRANTS, Q8, D-124) A classifier this gateway does not know never deserializes (unknown
    // `params` source or key), so the snapshot is refused and the previous one kept; a known one
    // must be well formed, so a `params` tool is never run as a path-only tool.
    for a in &s.apps {
        for g in a.tools.iter().flat_map(|t| [&t.read, &t.write]).flatten() {
            for t in &g.items {
                if let Some(p) = &t.params
                    && (p.is_empty() || p.len() > aurora_policy::params::MAX_CONDITIONS)
                {
                    return Err(format!(
                        "invalid params classifier on tool `{}` of `{}`",
                        t.id, a.app_id
                    ));
                }
            }
        }
    }
    for h in &s.hosts {
        if !HostPattern::lenient(&h.host).is_valid() {
            return Err(format!("invalid host pattern `{}`", h.host));
        }
        for i in &h.inject {
            if let Inject::PathRegex { pattern, .. } = i
                && regex::Regex::new(pattern).is_err()
            {
                return Err(format!("invalid path_regex on `{}`", h.host));
            }
        }
    }
    // `{instance}` entries belong to apps that declare `instances` (D-74).
    for h in &s.instance_hosts {
        if h.host != INSTANCE_HOST || s.app(&h.app_id).is_none_or(|a| a.instances.is_none()) {
            return Err(format!("invalid instance host entry of `{}`", h.app_id));
        }
        for i in &h.inject {
            if let Inject::PathRegex { pattern, .. } = i
                && regex::Regex::new(pattern).is_err()
            {
                return Err(format!(
                    "invalid path_regex on instance host of `{}`",
                    h.app_id
                ));
            }
        }
    }
    Ok(())
}

/// Parse and validate the stored JSON. Unknown mechanisms/decoders/finalizers fail
/// deserialization (closed enums) and are reported as refusals.
pub fn parse(hash: &str, body: serde_json::Value) -> Result<Loaded, String> {
    let snap: Snapshot = serde_json::from_value(body).map_err(|e| e.to_string())?;
    Loaded::new(hash, snap)
}

#[derive(Default)]
pub struct SnapshotStore {
    cur: ArcSwapOption<Loaded>,
    last_rejected: Mutex<Option<String>>,
    /// Wakes `run` for an immediate re-poll of the active hash (after a LISTEN reconnect).
    poke: Arc<tokio::sync::Notify>,
}

/// Retry period of the loader while no snapshot is loaded (the gateway is `down` until one is).
const LOAD_RETRY: Duration = Duration::from_secs(1);

impl SnapshotStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Notifier that makes the loader re-poll the active hash at once.
    pub fn poke(&self) -> Arc<tokio::sync::Notify> {
        self.poke.clone()
    }

    pub fn current(&self) -> Option<Arc<Loaded>> {
        self.cur.load_full()
    }

    /// `readyz`: false until the first snapshot is loaded.
    pub fn is_loaded(&self) -> bool {
        self.cur.load().is_some()
    }

    pub fn install(&self, l: Loaded) {
        self.cur.store(Some(Arc::new(l)));
    }

    /// Re-read the active snapshot; `Ok(true)` when it changed. A refused snapshot keeps
    /// the previous one (reported once per hash).
    pub async fn refresh(&self, pool: &PgPool) -> Result<bool, String> {
        let supported = i32::try_from(SUPPORTED_CONTRACT).unwrap_or(i32::MAX);
        let rows =
            aurora_db::repo::gateway::snapshot_headers(pool, (supported - 1).max(0), supported)
                .await
                .map_err(|e| e.to_string())?;
        let cands: Vec<(u32, _, String)> = rows
            .into_iter()
            .map(|r| {
                (
                    u32::try_from(r.contract).unwrap_or(0),
                    r.activated_at,
                    r.content_hash,
                )
            })
            .collect();
        let Some(hash) = select_active(&cands, SUPPORTED_CONTRACT) else {
            return Ok(false);
        };
        if self.current().is_some_and(|c| &*c.hash == hash) {
            return Ok(false);
        }
        if self
            .last_rejected
            .lock()
            .is_ok_and(|g| g.as_deref() == Some(hash))
        {
            return Ok(false);
        }
        let Some(body) = aurora_db::repo::gateway::snapshot_body(pool, hash)
            .await
            .map_err(|e| e.to_string())?
        else {
            return Ok(false);
        };
        match parse(hash, body) {
            Ok(l) => {
                tracing::info!(hash = %l.hash, "registry snapshot loaded");
                self.install(l);
                Ok(true)
            }
            Err(e) => {
                tracing::error!(hash, error = %e, "registry snapshot refused; keeping previous");
                metrics::counter!("gateway_snapshot_rejected_total").increment(1);
                if let Ok(mut g) = self.last_rejected.lock() {
                    *g = Some(hash.to_owned());
                }
                Err(e)
            }
        }
    }

    /// Load at start, then follow `aurora_registry` notifications plus a 30 s poll.
    pub async fn run(
        self: Arc<Self>,
        pool: PgPool,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        let mut listener = None;
        loop {
            if listener.is_none() {
                listener = Listener::connect(&pool, &[Channel::Registry]).await.ok();
            }
            let _ = self.refresh(&pool).await;
            // Until the first snapshot is loaded (started with the database down) retry fast.
            let wait = if self.is_loaded() {
                POLL_INTERVAL
            } else {
                LOAD_RETRY
            };
            tokio::select! {
                _ = crate::invalidation::stopped(&mut shutdown) => return,
                _ = tokio::time::sleep(wait) => {}
                _ = self.poke.notified() => {}
                ev = async {
                    match listener.as_mut() {
                        Some(l) => l.next().await,
                        None => std::future::pending().await,
                    }
                } => {
                    // Notifications are hints; the refresh at the loop head acts on them.
                    let _: BusEvent = ev;
                }
            }
        }
    }
}
