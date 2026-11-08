//! BYO admission (phase 2, RFE-025; docs/06 §BYO admission, 08 §11.9).
//!
//! Runs after the credential authenticated and before the lane is chosen, for every `CONNECT`
//! and absolute-form request, from data cached with the identity (no database on the hot path):
//!
//! 1. BYO posture `disabled` → 403 `byo_disabled`;
//! 2. the client address must lie inside the effective source list → 403 `source_not_permitted`.
//!
//! The third check (`context_credential_required`) needs injection selection and lives in the
//! pipeline (`decide.rs`). Live tunnels are re-evaluated on invalidation by [`TunnelRegistry`].

use crate::auth::{CredentialKind, CredentialResolver, RequestIdentity};
use crate::invalidation::{Invalidatable, Invalidate, Scope};
use aurora_core::ApiError;
use aurora_llm::governance::{ByoLevel, ByoPosture};
use http::StatusCode;
use serde_json::json;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::watch;
use uuid::Uuid;

/// Why a BYO credential was refused at admission (the `reason` label of
/// `aurora_gateway_byo_refused_total`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Posture,
    Source,
    ContextRequired,
}

impl Refusal {
    pub fn label(self) -> &'static str {
        match self {
            Self::Posture => "posture",
            Self::Source => "source",
            Self::ContextRequired => "context_required",
        }
    }
}

fn level_page(level: ByoLevel, external_url: &str, org: &str, ws: &str) -> String {
    let external_url = external_url.trim_end_matches('/');
    match level {
        ByoLevel::Workspace | ByoLevel::Agent => format!("{external_url}/o/{org}/w/{ws}/settings"),
        ByoLevel::Org | ByoLevel::Instance => format!("{external_url}/o/{org}/governance"),
    }
}

/// Admission checks 1 and 2. `Ok(())` for credentials that are not subject to BYO control.
pub fn admit(
    id: &RequestIdentity,
    client: IpAddr,
    external_url: &str,
) -> Result<(), (Refusal, ApiError)> {
    let Some(b) = &id.byo else { return Ok(()) };
    let e = &b.effective;
    if e.posture == ByoPosture::Disabled {
        let url = level_page(e.posture_level, external_url, &b.org_slug, &b.ws_slug);
        return Err((
            Refusal::Posture,
            ApiError::new(
                StatusCode::FORBIDDEN,
                "byo_disabled",
                format!(
                    "BYO agents are disabled by the {} settings. Review them at {url}",
                    e.posture_level.as_str()
                ),
            )
            .with_details(json!({ "level": e.posture_level.as_str(), "manage_url": url })),
        ));
    }
    if !e.permits_source(client) {
        let level = e.source_level.unwrap_or(ByoLevel::Agent);
        let url = level_page(level, external_url, &b.org_slug, &b.ws_slug);
        return Err((
            Refusal::Source,
            ApiError::new(
                StatusCode::FORBIDDEN,
                "source_not_permitted",
                format!(
                    "This credential may not be used from {client}: the agent's source list does not include it. Review it at {url}"
                ),
            )
            .with_details(json!({ "source": client.to_string(), "manage_url": url })),
        ));
    }
    Ok(())
}

/// 403 `context_credential_required` (admission check 3).
pub fn context_required(mint_url: &str) -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "context_credential_required",
        "This agent must use a context credential for requests that need a credential. Mint one with POST /v1/agent/context-credentials (the `aurora run` command does this for you).",
    )
    .with_details(json!({ "mint": format!("POST {mint_url}") }))
}

/// Whether the identity is an `aug_` credential of an agent that requires context credentials.
pub fn needs_context_credential(id: &RequestIdentity) -> bool {
    id.kind == CredentialKind::Agent && id.byo.as_ref().is_some_and(|b| b.effective.require_context)
}

/// One request-log row and one log line per credential per [`BYO_REFUSAL_LOG_INTERVAL`].
#[derive(Default)]
pub struct RefusalLimiter {
    last: Mutex<HashMap<Uuid, Instant>>,
}

impl RefusalLimiter {
    /// `true` when this refusal should be logged (the metric counts every refusal).
    pub fn should_log(&self, credential: Uuid) -> bool {
        let Ok(mut m) = self.last.lock() else {
            return true;
        };
        let now = Instant::now();
        if m.len() > 10_000 {
            m.retain(|_, t| now.duration_since(*t) < aurora_core::limits::BYO_REFUSAL_LOG_INTERVAL);
        }
        match m.get(&credential) {
            Some(t) if now.duration_since(*t) < aurora_core::limits::BYO_REFUSAL_LOG_INTERVAL => {
                false
            }
            _ => {
                m.insert(credential, now);
                true
            }
        }
    }
}

/// (P2-FU-2, D-97) The answer owed to requests still in the pipeline (held for approval, among
/// others) when a tunnel is closed because the credential became frozen or BYO-disabled.
#[derive(Debug, Clone)]
pub struct ClosedBy {
    /// The refusal a new request would get (403 `frozen` / 403 `byo_disabled`).
    pub error: ApiError,
    /// `Some(Posture)` for `byo_disabled`; `None` for `frozen`. Selects the refusal counter.
    pub reason: Option<Refusal>,
}

/// Carries the [`ClosedBy`] answer; always set before the kill signal fires.
pub type ClosedByRx = watch::Receiver<Option<Arc<ClosedBy>>>;

/// Resolves with the answer once the tunnel is closed by a freeze or a BYO tightening; never
/// resolves without a receiver or for other closes.
pub async fn closed_by(rx: &mut Option<ClosedByRx>) -> Arc<ClosedBy> {
    if let Some(r) = rx
        && let Ok(v) = r.wait_for(Option::is_some).await
        && let Some(c) = v.as_ref()
    {
        return c.clone();
    }
    std::future::pending().await
}

/// After an answered close, how long in-flight work may take to deliver the answer before the
/// connection is dropped regardless.
pub const CLOSE_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

struct Entry {
    credential: Uuid,
    agent: Uuid,
    workspace: Uuid,
    org: Uuid,
    source: IpAddr,
    /// `require_context` at connect time: turning it on closes the tunnel.
    require_context: bool,
    kill: watch::Sender<bool>,
    closed_by: watch::Sender<Option<Arc<ClosedBy>>>,
}

/// Live BYO tunnels. On an invalidation that concerns a tunnel the credential is re-resolved from
/// the database; the tunnel is closed (and with it every WebSocket and request held for approval)
/// when the credential is gone, the posture is now `disabled`, the source address is no longer
/// permitted, or context credentials became required. Loosening never closes anything.
pub struct TunnelRegistry {
    next: AtomicU64,
    live: Mutex<HashMap<u64, Entry>>,
    resolver: Arc<dyn CredentialResolver>,
    external_url: String,
}

/// Held by a tunnel for its lifetime; deregisters on drop.
pub struct TunnelGuard {
    registry: Arc<TunnelRegistry>,
    id: u64,
    kill: watch::Receiver<bool>,
    closed_by: ClosedByRx,
}

impl TunnelGuard {
    pub fn killed(&self) -> watch::Receiver<bool> {
        self.kill.clone()
    }

    /// The answer owed to requests in flight when the tunnel is closed (freeze / BYO posture).
    pub fn closed_by(&self) -> ClosedByRx {
        self.closed_by.clone()
    }
}

impl Drop for TunnelGuard {
    fn drop(&mut self) {
        if let Ok(mut l) = self.registry.live.lock() {
            l.remove(&self.id);
        }
    }
}

/// Resolves when the tunnel is told to close.
pub async fn killed(rx: &mut watch::Receiver<bool>) {
    let _ = rx.wait_for(|k| *k).await;
}

impl TunnelRegistry {
    pub fn new(resolver: Arc<dyn CredentialResolver>, external_url: &str) -> Arc<Self> {
        Arc::new(Self {
            next: AtomicU64::new(1),
            live: Mutex::new(HashMap::new()),
            resolver,
            external_url: external_url.trim_end_matches('/').to_owned(),
        })
    }

    pub fn len(&self) -> usize {
        self.live.lock().map(|l| l.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Registers a live tunnel. Every credential's tunnel is registered (phase 2, RFE-006): BYO
    /// admission changes (RFE-025) and freezes of the agent, its workspace or its organization
    /// (RFE-006) both close it on the next invalidation. Always `Some`.
    pub fn register(self: &Arc<Self>, id: &RequestIdentity, source: IpAddr) -> Option<TunnelGuard> {
        let require_context = id.byo.as_ref().is_some_and(|b| b.effective.require_context);
        let (tx, rx) = watch::channel(false);
        let (closed_tx, closed_rx) = watch::channel(None);
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut l) = self.live.lock() {
            l.insert(
                n,
                Entry {
                    credential: id.credential_id,
                    agent: id.agent_id,
                    workspace: id.workspace_id,
                    org: id.org_id,
                    source,
                    require_context,
                    kill: tx,
                    closed_by: closed_tx,
                },
            );
        }
        Some(TunnelGuard {
            registry: self.clone(),
            id: n,
            kill: rx,
            closed_by: closed_rx,
        })
    }

    fn recheck_all(&self, matches: impl Fn(&Entry) -> bool) {
        type Affected = (
            Uuid,
            IpAddr,
            bool,
            watch::Sender<bool>,
            watch::Sender<Option<Arc<ClosedBy>>>,
        );
        let affected: Vec<Affected> = match self.live.lock() {
            Ok(l) => l
                .values()
                .filter(|e| matches(e))
                .map(|e| {
                    (
                        e.credential,
                        e.source,
                        e.require_context,
                        e.kill.clone(),
                        e.closed_by.clone(),
                    )
                })
                .collect(),
            Err(_) => return,
        };
        for (credential, source, was_required, kill, closed_by) in affected {
            let resolver = self.resolver.clone();
            let external = self.external_url.clone();
            let task = async move {
                let mut answer: Option<ClosedBy> = None;
                let keep = match resolver.resolve_credential(credential).await {
                    // `admit` is a no-op for credentials without BYO control.
                    Ok(Some(id)) => match admit(&id, source, &external) {
                        Ok(()) => was_required || !needs_context_credential(&id),
                        // (P2-FU-2) A posture tightening answers what is still in flight.
                        Err((Refusal::Posture, error)) => {
                            answer = Some(ClosedBy {
                                error,
                                reason: Some(Refusal::Posture),
                            });
                            false
                        }
                        Err(_) => false,
                    },
                    // (RFE-006) The agent, its workspace or its org is frozen: close.
                    Err(crate::auth::ResolveError::Frozen { level }) => {
                        metrics::counter!("aurora_gateway_frozen_tunnels_closed_total")
                            .increment(1);
                        answer = Some(ClosedBy {
                            error: crate::guidance::frozen(level),
                            reason: None,
                        });
                        false
                    }
                    // Revoked, expired, gone, or the database is unreachable: fail closed.
                    _ => false,
                };
                if !keep {
                    metrics::counter!("aurora_gateway_byo_tunnels_closed_total").increment(1);
                    // The answer first, then the close: in-flight requests are refused before
                    // the connection goes (P2-FU-2).
                    if let Some(a) = answer {
                        let _ = closed_by.send(Some(Arc::new(a)));
                    }
                    let _ = kill.send(true);
                }
            };
            match tokio::runtime::Handle::try_current() {
                Ok(h) => {
                    h.spawn(task);
                }
                Err(_) => {
                    let _ = self.live.lock().map(|l| {
                        for e in l.values().filter(|e| e.credential == credential) {
                            let _ = e.kill.send(true);
                        }
                    });
                }
            }
        }
    }
}

impl Invalidatable for TunnelRegistry {
    fn invalidate(&self, ev: &Invalidate) {
        match ev.scope {
            Scope::Org(i) => self.recheck_all(|e| e.org == i),
            Scope::Workspace(i) => self.recheck_all(|e| e.workspace == i),
            Scope::Agent(i) => self.recheck_all(|e| e.agent == i),
            Scope::Credential(i) => self.recheck_all(|e| e.credential == i),
            Scope::All => self.recheck_all(|_| true),
            // Users and connections do not influence BYO admission.
            Scope::User(_) | Scope::Connection(_) => {}
        }
    }

    fn flush_all(&self) {
        self.recheck_all(|_| true);
    }
}
