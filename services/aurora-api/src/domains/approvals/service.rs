use super::bus::BusMsg;
use super::dto::{Approval, Decision};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::http::auth::{Caller, Principal};
use aurora_core::{ApiError, FieldError};
use aurora_db::authz;
use aurora_db::repo::approvals::{self, ApprovalRow};
use std::time::Duration;
use tokio::time::Instant;
use uuid::Uuid;

pub const STATUSES: [&str; 5] = ["pending", "approved", "denied", "expired", "abandoned"];

/// Key confinement as `(org, workspace)` filters; sessions are unconfined.
pub fn scope(c: &Caller) -> (Option<Uuid>, Option<Uuid>) {
    match &c.principal {
        Principal::ApiKey {
            org_id,
            workspace_id,
            ..
        } => (Some(*org_id), *workspace_id),
        Principal::Session { .. } | Principal::Channel { .. } => (None, None),
    }
}

fn via(c: &Caller) -> &'static str {
    c.audit_source()
}

pub fn validate_status(status: Option<&str>) -> Result<(), ApiError> {
    match status {
        Some(s) if !STATUSES.contains(&s) => Err(ApiError::invalid_request(
            "Invalid status.",
            vec![FieldError::new("/status", "unknown status")],
        )),
        _ => Ok(()),
    }
}

fn decision_event(c: &Caller, row: &ApprovalRow, decision: Decision, batch: bool) -> AuditEvent {
    let verb = match decision {
        Decision::Approve => "approved",
        Decision::Deny => "denied",
    };
    AuditEvent::new(
        AuditActor::from_caller(c),
        "approval.decide",
        Outcome::Success,
    )
    .org(row.org_id)
    .workspace(row.workspace_id)
    .target("approval", row.id)
    .metadata(serde_json::json!({
        "decision": verb, "via": via(c), "batch": batch,
        "agent_id": row.agent_id, "host": row.host, "method": row.method,
    }))
}

/// Seam for M4/M6: append `approval.decided` transcript events and update channel cards.
fn after_decision(_s: &AppState, _row: &ApprovalRow) {}

pub async fn decide(
    s: &AppState,
    c: &Caller,
    id: Uuid,
    decision: Decision,
) -> Result<Approval, ApiError> {
    let existing = approvals::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Approval not found."))?;
    // 404 when the workspace is invisible; key confinement applies.
    let auth = c.workspace(s, existing.workspace_id).await?;
    if auth.org_id != existing.org_id || !auth.can_use() {
        return Err(ApiError::not_found("Approval not found."));
    }
    // RFE-009: an approval naming a direct conversation the caller cannot see does not exist.
    if !approvals::conversation_visible(
        &s.pool,
        c.user_id,
        existing.workspace_id,
        existing.conversation_id,
    )
    .await?
    {
        return Err(ApiError::not_found("Approval not found."));
    }
    // RFE-012 (D-55, SEC-28): only `can_approve` decides; a plain member gets 403.
    crate::http::auth::require_approve(&auth)?;
    let mut tx = audit::begin(s).await?;
    let row = approvals::decide(
        &mut tx,
        id,
        decision == Decision::Approve,
        c.user_id,
        via(c),
    )
    .await?
    .ok_or_else(|| {
        ApiError::conflict(
            "already_decided",
            "This approval has already been resolved.",
        )
    })?;
    // RFE-008: the decision commits with its audit row, or does not happen.
    audit::commit_critical(tx, &decision_event(c, &row, decision, false)).await?;
    after_decision(s, &row);
    Ok(row.into())
}

pub async fn decide_batch(
    s: &AppState,
    c: &Caller,
    batch_id: &str,
    agent_id: Uuid,
    decision: Decision,
) -> Result<u32, ApiError> {
    let (org, ws) = scope(c);
    if !approvals::batch_visible(&s.pool, c.user_id, org, ws, agent_id, batch_id).await? {
        // RFE-012: a plain member who can see the batch is told it needs an approver.
        if approvals::batch_undecidable(&s.pool, c.user_id, org, ws, agent_id, batch_id).await? {
            return Err(crate::http::auth::approver_required());
        }
        return Err(ApiError::not_found("Batch not found."));
    }
    let mut tx = audit::begin(s).await?;
    let rows = approvals::decide_batch(
        &mut tx,
        c.user_id,
        org,
        ws,
        agent_id,
        batch_id,
        decision == Decision::Approve,
        via(c),
    )
    .await?;
    // RFE-008: every decided row commits with its audit row (all or nothing).
    let events: Vec<AuditEvent> = rows
        .iter()
        .map(|(id, workspace_id, org_id)| {
            AuditEvent::new(AuditActor::from_caller(c), "approval.decide", Outcome::Success)
                .org(*org_id)
                .workspace(*workspace_id)
                .target("approval", id)
                .metadata(serde_json::json!({
                    "decision": match decision { Decision::Approve => "approved", Decision::Deny => "denied" },
                    "via": via(c), "batch": true, "batch_id": batch_id, "agent_id": agent_id,
                }))
        })
        .collect();
    audit::commit_critical_many(tx, &events).await?;
    Ok(u32::try_from(rows.len()).unwrap_or(u32::MAX))
}

/// Pending approvals the caller may decide; waits up to `wait` seconds (<=30)
/// for a new one when there are none.
pub async fn pending(s: &AppState, c: &Caller, wait: u32) -> Result<Vec<Approval>, ApiError> {
    let (org, ws) = scope(c);
    let wait =
        Duration::from_secs(u64::from(wait).min(aurora_core::limits::LONG_POLL_MAX.as_secs()));
    // Subscribe before the first read so a concurrent create cannot be missed.
    let mut rx = if wait.is_zero() {
        None
    } else {
        Some(s.approval_bus.subscribe(&s.pool).await?)
    };
    // (RFE-011a) A held-open long-poll is a live delivery surface for its user.
    let _watch = if wait.is_zero() {
        None
    } else {
        Some(super::notify::watch(s, c.user_id, org, ws).await)
    };
    let read = || async { approvals::list_pending_for(&s.pool, c.user_id, org, ws, 200).await };
    let rows = read().await?;
    let Some(rx) = rx.as_mut() else {
        return Ok(rows.into_iter().map(Into::into).collect());
    };
    if !rows.is_empty() {
        return Ok(rows.into_iter().map(Into::into).collect());
    }
    let deadline = Instant::now() + wait;
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Err(_) | Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                return Ok(vec![]);
            }
            Ok(Ok(BusMsg::Event { event, .. })) if event != "created" => continue,
            Ok(_) => {
                let rows = read().await?;
                if !rows.is_empty() {
                    return Ok(rows.into_iter().map(Into::into).collect());
                }
            }
        }
    }
}

/// (RFE-011a) The sanitized notification facts of a gateway approval: no summary fields, no body.
pub async fn notice_for(s: &AppState, a: &Approval) -> Option<super::dto::ApprovalNotice> {
    let (name, slug, org_slug) =
        aurora_db::repo::approval_notify::workspace_labels(&s.pool, a.workspace_id)
            .await
            .ok()
            .flatten()?;
    Some(super::dto::ApprovalNotice {
        id: a.id,
        workspace: super::dto::ApprovalNoticeWorkspace {
            id: a.workspace_id,
            name,
            slug,
            org_slug,
        },
        agent: a.agent.as_ref().map(|g| super::dto::ApprovalAgent {
            id: g.id,
            name: g.name.clone(),
            handle: g.handle.clone(),
        }),
        title: aurora_core::sanitize::one_line(&a.summary.title, 120),
        kind: "gateway".to_owned(),
        expires_at: a.deadline,
    })
}

/// Resolve one bus event for an SSE subscriber: `None` when the user may not see it.
pub async fn stream_item(
    s: &AppState,
    user: Uuid,
    id: Uuid,
    workspace_id: Uuid,
) -> Option<Approval> {
    match authz::can_approve(&s.pool, user, workspace_id).await {
        Ok(true) => {}
        Ok(false) => return None,
        Err(e) => {
            tracing::warn!(error = %e, "approval stream authz failed");
            return None;
        }
    }
    match approvals::get(&s.pool, id).await {
        Ok(Some(row)) => {
            match approvals::conversation_visible(
                &s.pool,
                user,
                row.workspace_id,
                row.conversation_id,
            )
            .await
            {
                Ok(true) => Some(row.into()),
                Ok(false) => None,
                Err(e) => {
                    tracing::warn!(error = %e, "approval stream visibility failed");
                    None
                }
            }
        }
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(error = %e, "approval stream load failed");
            None
        }
    }
}
