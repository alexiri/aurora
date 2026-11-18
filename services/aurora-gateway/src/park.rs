//! Parking and single-use grants (docs/06 §Parking and grants, docs/07 §Park and resume, SEC-74,
//! C-820, C-821, C-827; P4-011B-1).
//!
//! A request that needs approval and whose deadline is long is **refused at once and never sent**:
//! the approval stays `pending` in Postgres with a salted fingerprint of the exact request. Once a
//! person approves (the API writes `resume_state = 'ready'`), the **identical** request is let
//! through exactly once: [`Holds::consume_grant`] recomputes the fingerprint with each open
//! grant's salt and consumes the match with one compare-and-set. The request itself (body,
//! headers, query values) is never stored or logged.

use crate::hold::{HoldRequest, Holds};
use crate::{Body, BoxError};
use arc_swap::ArcSwap;
use aurora_db::repo::gateway::{self as repo, GrantRow, ParkLimits, ParkOutcome};
use aurora_policy::park::{self, Inputs};
use bytes::{Bytes, BytesMut};
use http_body_util::BodyExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;
use uuid::Uuid;

/// The live `approvals.park.*` values the gateway reads per request (C-829). Replaced on an
/// `approvals` settings swap; a hold or park already waiting keeps what it was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParkConfig {
    /// Governs only whether **new** approvals park; grant lookup and consumption ignore it.
    pub enabled: bool,
    pub threshold_secs: u64,
    pub max_timeout_secs: u64,
    pub resume_window_secs: u64,
    pub deny_cooldown_secs: u64,
    pub max_body_bytes: usize,
    pub max_pending_per_agent: u32,
}

impl Default for ParkConfig {
    fn default() -> Self {
        Self::from(&aurora_config::ApprovalsParkSection::default())
    }
}

impl From<&aurora_config::ApprovalsParkSection> for ParkConfig {
    fn from(p: &aurora_config::ApprovalsParkSection) -> Self {
        Self {
            enabled: p.enabled,
            threshold_secs: p.threshold_secs,
            max_timeout_secs: p.max_timeout_secs,
            resume_window_secs: p.resume_window_secs,
            deny_cooldown_secs: p.deny_cooldown_secs,
            max_body_bytes: usize::try_from(p.max_body_bytes).unwrap_or(usize::MAX),
            max_pending_per_agent: p.max_pending_per_agent,
        }
    }
}

/// The shared holder of the live [`ParkConfig`].
pub struct ParkSettings(ArcSwap<ParkConfig>);

impl ParkSettings {
    pub fn new(c: ParkConfig) -> Self {
        Self(ArcSwap::from_pointee(c))
    }
    pub fn load(&self) -> Arc<ParkConfig> {
        self.0.load_full()
    }
    pub fn store(&self, c: ParkConfig) {
        self.0.store(Arc::new(c));
    }
}

/// What the grant lookup did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantOutcome {
    /// An open grant matched and was consumed by this request.
    Consumed(Uuid),
    /// Nothing to consume: continue with a park or a hold.
    None,
    /// The store failed: the request is refused (fail closed), nothing is parked or consumed.
    StoreUnavailable,
}

/// What a park attempt did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParkAnswer {
    Parked {
        id: Uuid,
        expires_at: chrono::DateTime<chrono::Utc>,
    },
    /// An identical request is already pending: the same answer, nothing inserted.
    Existing {
        id: Uuid,
        expires_at: chrono::DateTime<chrono::Utc>,
    },
    Cooldown {
        id: Uuid,
        until: chrono::DateTime<chrono::Utc>,
    },
    Limit {
        limit: u32,
        per: &'static str,
    },
    StoreUnavailable,
}

/// A request to park: the approval as a hold would record it and the fingerprint inputs.
pub struct ParkRequest<'a> {
    pub hold: HoldRequest<'a>,
    pub inputs: &'a Inputs<'a>,
}

impl Holds {
    /// The live park configuration.
    pub fn park_config(&self) -> Arc<ParkConfig> {
        self.park.load()
    }

    /// Replaces the park configuration (settings swap).
    pub fn set_park(&self, c: ParkConfig) {
        self.park.store(c);
    }

    /// (RFE-011a, C-820) The effective timeout of a **parked** approval: the rule-level minimum
    /// clamped to `[APPROVAL_TIMEOUT_MIN, approvals.park.max_timeout_secs]`, else the default.
    pub fn park_timeout(&self, rule_timeout_secs: Option<u32>) -> Duration {
        let cfg = self.park.load();
        let default = self.default_timeout().as_secs();
        Duration::from_secs(aurora_core::limits::effective_approval_timeout_secs(
            rule_timeout_secs,
            default,
            self.min_timeout.as_secs(),
            cfg.max_timeout_secs,
        ))
    }

    /// Grant lookup (SEC-74): the open grants of this agent and conversation; the caller reads the
    /// body only when there are some, recomputes each fingerprint with its own salt
    /// ([`Holds::consume_from`]) and consumes a match by one compare-and-set. Runs on every
    /// `needs_approval` verdict, whatever `approvals.park.enabled` says now (D-264(b)).
    pub async fn open_grants(
        &self,
        agent_id: Uuid,
        conversation_id: Uuid,
    ) -> Option<Vec<GrantRow>> {
        if self.db_down() {
            // A request that needs approval cannot be held or parked now either.
            return Some(Vec::new());
        }
        match repo::ready_grants(&self.pool, agent_id, conversation_id).await {
            Ok(r) => Some(r),
            Err(e) => {
                tracing::warn!(error = %e, "grant lookup failed");
                self.report(&e);
                None
            }
        }
    }

    /// Consumes the grant of `rows` (from [`Holds::open_grants`]) whose fingerprint matches
    /// `inputs`, with one compare-and-set.
    pub async fn consume_from(&self, rows: &[GrantRow], inputs: &Inputs<'_>) -> GrantOutcome {
        let Some(row) = rows
            .iter()
            .find(|r| park::matches(&r.fingerprint, &r.fp_salt, inputs))
        else {
            metrics::counter!("aurora_gateway_approval_grants_total", "result" => "no_match")
                .increment(1);
            return GrantOutcome::None;
        };
        match repo::consume_grant(&self.pool, row.id).await {
            Ok(true) => {
                metrics::counter!("aurora_gateway_approval_grants_total", "result" => "consumed")
                    .increment(1);
                GrantOutcome::Consumed(row.id)
            }
            // Another retry consumed it first, or its window ended between read and update.
            Ok(false) => {
                metrics::counter!("aurora_gateway_approval_grants_total", "result" => "expired")
                    .increment(1);
                GrantOutcome::None
            }
            Err(e) => {
                tracing::warn!(error = %e, approval_id = %row.id, "consuming a grant failed");
                self.report(&e);
                GrantOutcome::StoreUnavailable
            }
        }
    }

    /// Parks the request (C-820): de-duplication, the deny cooldown and the per-agent caps are
    /// decided in one transaction with the insert; the `created` event follows the commit.
    pub async fn park(&self, r: ParkRequest<'_>) -> ParkAnswer {
        if self.db_down() {
            return ParkAnswer::StoreUnavailable;
        }
        let Some(salt) = park::new_salt() else {
            tracing::error!("the operating system refused random bytes; nothing is parked");
            return ParkAnswer::StoreUnavailable;
        };
        let fp = park::fingerprint(&salt, r.inputs);
        let cfg = self.park.load();
        let limits = ParkLimits {
            max_pending_per_agent: cfg.max_pending_per_agent,
            per_agent_hour: aurora_core::limits::PARK_PER_AGENT_HOUR,
            deny_cooldown_secs: cfg.deny_cooldown_secs as f64,
        };
        let inputs = *r.inputs;
        let matches = move |s: &[u8], f: &[u8]| park::matches(f, s, &inputs);
        let id = Uuid::now_v7();
        let ws = r.hold.identity.workspace_id;
        let timeout = self.park_timeout(r.hold.rule_timeout_secs);
        let Some(row) = self.approval_row(id, &r.hold, timeout) else {
            return ParkAnswer::StoreUnavailable;
        };
        match repo::insert_parked(&self.pool, &row, &salt, &fp, limits, &matches).await {
            Ok(ParkOutcome::Parked {
                id: pid,
                expires_at,
            }) => {
                metrics::counter!("aurora_gateway_approvals_parked_total").increment(1);
                self.emit(id, ws, "created").await;
                ParkAnswer::Parked {
                    id: pid,
                    expires_at,
                }
            }
            Ok(ParkOutcome::Existing { id, expires_at }) => ParkAnswer::Existing { id, expires_at },
            Ok(ParkOutcome::Cooldown { id, until }) => ParkAnswer::Cooldown { id, until },
            Ok(ParkOutcome::Limit { limit, per }) => ParkAnswer::Limit { limit, per },
            Err(e) => {
                tracing::error!(error = %e, "parking an approval failed");
                self.report(&e);
                ParkAnswer::StoreUnavailable
            }
        }
    }
}

/// The result of [`buffer_whole`].
pub struct Buffered {
    /// The complete body, when it ended within the cap and the time budget.
    pub whole: Option<Bytes>,
}

/// Reads the **complete** body when it is at most `max` bytes and arrives within `within`;
/// whatever happens the `body` is replaced by one that replays everything read so far followed by
/// the rest, so a request that cannot be parked is still held and forwarded intact. Never stores
/// or logs a byte of it.
pub async fn buffer_whole(body: &mut Body, max: usize, within: Duration) -> Buffered {
    let mut taken = std::mem::replace(body, crate::empty_body());
    let deadline = Instant::now() + within;
    let mut buf = BytesMut::new();
    let mut ended = false;
    let mut failed: Option<BoxError> = None;
    while buf.len() <= max {
        match tokio::time::timeout_at(deadline, taken.frame()).await {
            Err(_) => break,
            Ok(None) => {
                ended = true;
                break;
            }
            Ok(Some(Ok(f))) => {
                if let Ok(d) = f.into_data() {
                    buf.extend_from_slice(&d);
                }
            }
            Ok(Some(Err(e))) => {
                failed = Some(e);
                break;
            }
        }
    }
    let all = buf.freeze();
    let whole = (ended && all.len() <= max).then(|| all.clone());
    // (FIX-166) A body read to its end keeps its framing: empty stays empty, a whole body is
    // `Content-Length`-sized.
    *body = crate::decide::replay_body(all, ended, failed, taken);
    Buffered { whole }
}

/// The guidance text of a parked refusal (docs/07 §What the agent sees), with the window in
/// minutes.
pub fn parked_message(resume_window_secs: u64) -> String {
    let minutes = resume_window_secs.div_ceil(60).max(1);
    format!(
        "This request needs a person's approval and that may take hours. It was not sent. Do not retry it now and do not look for another way around it. Finish what you can without it, tell the user what you are waiting for, and end your turn. Aurora will message you in this conversation when it is decided; if approved, repeat exactly this request once within {minutes} minutes."
    )
}
