//! Audit trail (docs/04 §Audit). Two classes (RFE-008, D-52): the security-critical set is
//! written in the change's transaction with [`commit_critical`] and fails the action when the row
//! cannot be written; everything else uses [`record`] (best-effort). `metadata` must never
//! contain secrets, tokens or credential JSON.

use crate::app::AppState;
use crate::http::auth::{Caller, KeyKind, Principal};
use aurora_core::ApiError;
use aurora_db::repo::audit::{self, NewAudit};
use serde_json::{Value, json};
use std::net::IpAddr;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    Failure,
    Denied,
}

impl Outcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Denied => "denied",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuditActor {
    /// `user` | `service` | `agent` | `system` | `instance_admin`.
    pub kind: &'static str,
    pub id: Option<Uuid>,
    pub label: String,
    /// `dashboard` | `api` | `cli` | `channel` | `system`.
    pub source: &'static str,
    pub ip: Option<IpAddr>,
}

impl AuditActor {
    pub fn from_caller(c: &Caller) -> Self {
        let kind = match c.principal {
            Principal::ApiKey {
                kind: KeyKind::Service,
                ..
            } => "service",
            _ => "user",
        };
        Self {
            kind,
            id: Some(c.user_id),
            label: c.email.clone(),
            source: c.audit_source(),
            ip: c.ip,
        }
    }

    /// Instance-admin action performed by a session caller.
    pub fn instance_admin(c: &Caller) -> Self {
        Self {
            kind: "instance_admin",
            ..Self::from_caller(c)
        }
    }

    pub fn system(label: impl Into<String>) -> Self {
        Self {
            kind: "system",
            id: None,
            label: label.into(),
            source: "system",
            ip: None,
        }
    }

    #[must_use]
    pub fn with_source(mut self, source: &'static str) -> Self {
        self.source = source;
        self
    }
}

#[derive(Debug, Clone)]
pub struct AuditEvent {
    pub org_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub actor: AuditActor,
    /// `entity.verb`.
    pub action: String,
    pub target_kind: Option<String>,
    pub target_id: Option<String>,
    pub outcome: Outcome,
    pub metadata: Value,
}

impl AuditEvent {
    pub fn new(actor: AuditActor, action: impl Into<String>, outcome: Outcome) -> Self {
        Self {
            org_id: None,
            workspace_id: None,
            actor,
            action: action.into(),
            target_kind: None,
            target_id: None,
            outcome,
            metadata: json!({}),
        }
    }
    #[must_use]
    pub fn org(mut self, id: Uuid) -> Self {
        self.org_id = Some(id);
        self
    }
    #[must_use]
    pub fn workspace(mut self, id: Uuid) -> Self {
        self.workspace_id = Some(id);
        self
    }
    #[must_use]
    pub fn target(mut self, kind: impl Into<String>, id: impl ToString) -> Self {
        self.target_kind = Some(kind.into());
        self.target_id = Some(id.to_string());
        self
    }
    #[must_use]
    pub fn metadata(mut self, m: Value) -> Self {
        self.metadata = m;
        self
    }
}

/// Insert and return the error (for tests and callers that want to await).
pub async fn insert(state: &AppState, ev: &AuditEvent) -> Result<Uuid, aurora_db::DbError> {
    audit::insert(&state.pool, &new_audit(ev)).await
}

/// Insert inside the caller's transaction: for security-critical actions the row commits or
/// rolls back with the change (docs/04 §Audit).
pub async fn insert_tx(
    conn: &mut sqlx::PgConnection,
    ev: &AuditEvent,
) -> Result<Uuid, aurora_db::DbError> {
    audit::insert(&mut *conn, &new_audit(ev)).await
}

fn new_audit(ev: &AuditEvent) -> NewAudit<'_> {
    NewAudit {
        org_id: ev.org_id,
        workspace_id: ev.workspace_id,
        actor_kind: ev.actor.kind,
        actor_id: ev.actor.id,
        actor_label: &ev.actor.label,
        action: &ev.action,
        target_kind: ev.target_kind.as_deref(),
        target_id: ev.target_id.as_deref(),
        outcome: ev.outcome.as_str(),
        source: ev.actor.source,
        ip: ev.actor.ip,
        metadata: &ev.metadata,
    }
}

/// The 503 a critical action returns when its audit row cannot be written ("no audit, no
/// change", docs/04 §Audit).
pub fn unavailable() -> ApiError {
    ApiError::unavailable("audit_unavailable", "The audit log is unavailable.")
}

/// A transaction that carries a security-critical change. Finish it with [`commit_critical`].
pub type Tx = sqlx::Transaction<'static, sqlx::Postgres>;

/// Begin the transaction of a security-critical action.
pub async fn begin(state: &AppState) -> Result<Tx, ApiError> {
    Ok(state.pool.begin().await.map_err(aurora_db::DbError::from)?)
}

/// Write the critical audit row inside `tx` and commit both. A failed audit insert drops (rolls
/// back) the transaction and returns 503 `audit_unavailable`: the change does not happen.
/// This is the only way critical handlers commit; the coverage test (`tests/it/audit_atomic.rs`)
/// proves each listed action goes through it.
pub async fn commit_critical(tx: Tx, ev: &AuditEvent) -> Result<(), ApiError> {
    commit_critical_many(tx, std::slice::from_ref(ev)).await
}

/// [`commit_critical`] for a change that is audited as several rows (e.g. a batch decision).
pub async fn commit_critical_many(mut tx: Tx, evs: &[AuditEvent]) -> Result<(), ApiError> {
    for ev in evs {
        debug_assert!(
            ev.outcome != Outcome::Success || aurora_core::audit::is_critical(&ev.action),
            "{} is not in SECURITY_CRITICAL_ACTIONS",
            ev.action
        );
        if let Err(e) = insert_tx(&mut tx, ev).await {
            tracing::error!(error = %e, action = %ev.action, "critical audit write failed");
            metrics::counter!("aurora_audit_critical_failed_total").increment(1);
            return Err(unavailable());
        }
    }
    tx.commit().await.map_err(aurora_db::DbError::from)?;
    Ok(())
}

/// Fire-and-forget audit write for the best-effort class; failures are logged and counted,
/// never propagated. Security-critical successes must use [`commit_critical`] instead.
pub fn record(state: &AppState, ev: AuditEvent) {
    debug_assert!(
        ev.outcome != Outcome::Success || !aurora_core::audit::is_critical(&ev.action),
        "{} is security-critical: write it with audit::commit_critical",
        ev.action
    );
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = insert(&state, &ev).await {
            tracing::error!(error = %e, action = %ev.action, "audit write failed");
            metrics::counter!("aurora_audit_dropped_total").increment(1);
        }
    });
}
