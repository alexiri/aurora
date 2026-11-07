//! Best-effort audit trail (docs/04 §Audit). Never fails the action; `metadata`
//! must never contain secrets, tokens or credential JSON.

use crate::app::AppState;
use crate::http::auth::{Caller, KeyKind, Principal};
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
    audit::insert(
        &state.pool,
        &NewAudit {
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
        },
    )
    .await
}

/// Fire-and-forget audit write; failures are logged, never propagated.
pub fn record(state: &AppState, ev: AuditEvent) {
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = insert(&state, &ev).await {
            tracing::error!(error = %e, action = %ev.action, "audit write failed");
        }
    });
}
