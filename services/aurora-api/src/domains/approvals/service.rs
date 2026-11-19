use super::bus::BusMsg;
use super::dto::{AlwaysApplied, AlwaysTool, Approval, Decision};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::grants::levels::{self, Level};
use crate::http::auth::{Caller, Principal};
use aurora_core::ApiError;
use aurora_db::authz;
use aurora_db::repo::approvals::{self, ApprovalRow, RuleOrigin};
use aurora_db::repo::connections;
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
        Some(s) if !STATUSES.contains(&s) => Err(ApiError::invalid_field(
            "Invalid status.",
            "/status",
            "unknown status",
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
        // (P4-011B-2, C-838) a parked approval's grant window; absent for held approvals.
        "parked": row.parked, "resume_expires_at": row.resume_expires_at,
    }))
}

fn already_decided() -> ApiError {
    ApiError::conflict(
        "already_decided",
        "This approval has already been resolved.",
    )
}

/// (C-982) `422 always_not_available`: Always allow was asked for where SEC-131 does not allow it.
fn always_not_available(why: &str) -> ApiError {
    ApiError::unprocessable("always_not_available", why)
}

/// (C-982, SEC-131) The Always allow part of a decision, in the deciding transaction: checks that
/// the question was asked by the agent's access, that the caller owns the workspace, and that an
/// organization's account allows changes without asking; then moves exactly the asked tool from
/// asked to allowed in the agent's grant and approves the other pending approvals of the same
/// agent, account and tool. Nothing is written when a check fails.
async fn apply_always(
    s: &AppState,
    c: &Caller,
    tx: &mut audit::Tx,
    row: &ApprovalRow,
) -> Result<(AlwaysApplied, Vec<ApprovalRow>, Vec<AuditEvent>), ApiError> {
    if !matches!(c.principal, Principal::Session { .. }) {
        return Err(always_not_available(
            "Always allow can only be chosen in the dashboard.",
        ));
    }
    if !authz::can_own(&mut **tx, c.user_id, row.workspace_id).await? {
        return Err(always_not_available(
            "Only a workspace owner can choose Always allow.",
        ));
    }
    let (Some(logical), Some(connection_id), Some(tool_id)) = (
        row.rule_logical_id,
        row.connection_id,
        row.tool_id.as_deref(),
    ) else {
        return Err(always_not_available(
            "This question is not about one change of one account.",
        ));
    };
    let Some(RuleOrigin::Access {
        agent_id,
        connection_id: grant_conn,
        ..
    }) = approvals::rule_origin(tx, logical).await?
    else {
        return Err(always_not_available(
            "A rule asked this question, not the agent's access.",
        ));
    };
    if agent_id != row.agent_id || grant_conn != connection_id {
        return Err(always_not_available(
            "A rule asked this question, not the agent's access.",
        ));
    }
    let conn = connections::get(&mut **tx, connection_id)
        .await?
        .ok_or_else(|| always_not_available("The account is gone."))?;
    if conn.workspace_id.is_none()
        && levels::connection_limit(conn.limit_level.as_deref(), &conn.access) != Level::Act
    {
        return Err(always_not_available(
            "This account's limit does not allow changes without asking.",
        ));
    }
    let written = crate::domains::grants::service::always_allow_tool_in_tx(
        tx,
        s,
        c,
        agent_id,
        connection_id,
        tool_id,
        row.id,
    )
    .await?
    .ok_or_else(|| always_not_available("The agent's access no longer asks about this change."))?;
    let name = {
        let snap = crate::domains::plugins::registry::active_typed_on(tx).await?;
        snap.as_ref()
            .and_then(|sn| sn.app(&conn.app_id))
            .map(crate::domains::apps::service::tools)
            .and_then(|ts| ts.into_iter().find(|t| t.id == tool_id))
            .map_or_else(|| tool_id.to_owned(), |t| t.name)
    };
    let others = approvals::approve_same_kind(
        tx,
        row.id,
        agent_id,
        connection_id,
        tool_id,
        c.user_id,
        via(c),
        s.ops().approvals.park.resume_window_secs,
    )
    .await?;
    let applied = AlwaysApplied {
        tool: AlwaysTool {
            id: tool_id.to_owned(),
            name,
        },
        also_approved: u32::try_from(others.len()).unwrap_or(u32::MAX),
    };
    Ok((applied, others, written.event.into_iter().collect()))
}

pub async fn decide(
    s: &AppState,
    c: &Caller,
    id: Uuid,
    decision: Decision,
    always: bool,
) -> Result<Approval, ApiError> {
    if always && decision != Decision::Approve {
        return Err(always_not_available("Always allow goes with approving."));
    }
    let existing = approvals::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Approval not found."))?;
    // 404 when the workspace is invisible; key confinement applies.
    let auth = c.workspace(s, existing.workspace_id).await?;
    if auth.org_id != existing.org_id {
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
    // (FIX-109, 07 §Approval deadlines) A decision after the deadline is refused and the
    // approval is expired in this same transaction (committed even though the answer is 409),
    // whether or not the 10 s sweep has run yet.
    let expired = approvals::expire_if_overdue(&mut tx, id).await?;
    let row = approvals::decide(
        &mut tx,
        id,
        decision == Decision::Approve,
        c.user_id,
        via(c),
        s.ops().approvals.park.resume_window_secs,
    )
    .await?;
    let Some(row) = row else {
        if expired {
            tx.commit().await.map_err(aurora_db::DbError::from)?;
        }
        return Err(already_decided());
    };
    // (P4-011B-2, C-823/C-824) The follow-up of a parked approval is written in this transaction;
    // a failure rolls the decision back with it.
    super::resume::after_decide(
        &mut tx,
        s,
        std::slice::from_ref(&row),
        decision == Decision::Approve,
    )
    .await?;
    // (C-982, SEC-131) Always allow: the access change, the other pending approvals of the same
    // kind and their audit rows commit with this decision, or none of it happens.
    let mut applied = None;
    let mut extra_events: Vec<AuditEvent> = Vec::new();
    if always {
        let (a, others, events) = apply_always(s, c, &mut tx, &row).await?;
        super::resume::after_decide(&mut tx, s, &others, true).await?;
        for o in &others {
            extra_events.push(decision_event(c, o, Decision::Approve, false));
            super::transcript::write(s, &mut tx, o, super::transcript::Kind::Pending, None).await?;
            super::transcript::write(s, &mut tx, o, super::transcript::Kind::Decided, None).await?;
        }
        extra_events.extend(events);
        applied = Some(a);
    }
    // A frozen agent or a gone conversation turned the grant `revoked`: answer what is stored.
    let row = approvals::get(&mut *tx, id).await?.unwrap_or(row);
    // (C-983) The answer appears in the conversation where the agent paused.
    super::transcript::write(s, &mut tx, &row, super::transcript::Kind::Pending, None).await?;
    super::transcript::write(
        s,
        &mut tx,
        &row,
        super::transcript::Kind::Decided,
        applied.as_ref(),
    )
    .await?;
    // RFE-008: the decision commits with its audit row, or does not happen.
    let mut events = vec![decision_event(c, &row, decision, false)];
    events.extend(extra_events);
    audit::commit_critical_many(tx, &events).await?;
    let mut out = Approval::from(row).with_resume();
    out.always_applied = applied;
    super::enrich::enrich(
        s,
        is_session(c).then_some(c.user_id),
        std::slice::from_mut(&mut out),
    )
    .await;
    Ok(out)
}

fn is_session(c: &Caller) -> bool {
    matches!(c.principal, Principal::Session { .. })
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
            return Err(crate::http::auth::approver_required(org, ws));
        }
        return Err(ApiError::not_found("Batch not found."));
    }
    let mut tx = audit::begin(s).await?;
    // (FIX-109) Overdue members of the batch are expired here, not decided.
    let expired =
        approvals::expire_overdue_of_batch(&mut tx, c.user_id, org, ws, agent_id, batch_id).await?;
    let rows = approvals::decide_batch(
        &mut tx,
        c.user_id,
        org,
        ws,
        agent_id,
        batch_id,
        decision == Decision::Approve,
        via(c),
        s.ops().approvals.park.resume_window_secs,
    )
    .await?;
    if rows.is_empty() && expired > 0 {
        // Nothing left to decide: the batch ran out of time. The expiry commits; the call is 409.
        tx.commit().await.map_err(aurora_db::DbError::from)?;
        return Err(already_decided());
    }
    // (P4-011B-2, C-824) One follow-up per conversation for the parked members of the batch.
    let mut decided_rows = Vec::new();
    for (id, _, _) in &rows {
        if let Some(r) = approvals::get(&mut *tx, *id).await? {
            decided_rows.push(r);
        }
    }
    super::resume::after_decide(&mut tx, s, &decided_rows, decision == Decision::Approve).await?;
    // (C-983) Each decided row appears in its conversation.
    for r in &decided_rows {
        super::transcript::write(s, &mut tx, r, super::transcript::Kind::Pending, None).await?;
        super::transcript::write(s, &mut tx, r, super::transcript::Kind::Decided, None).await?;
    }
    // RFE-008: every decided row commits with its audit row (all or nothing).
    let events: Vec<AuditEvent> = rows
        .iter()
        .map(|(id, workspace_id, org_id)| {
            let stored = decided_rows.iter().find(|r| r.id == *id);
            AuditEvent::new(AuditActor::from_caller(c), "approval.decide", Outcome::Success)
                .org(*org_id)
                .workspace(*workspace_id)
                .target("approval", id)
                .metadata(serde_json::json!({
                    "decision": match decision { Decision::Approve => "approved", Decision::Deny => "denied" },
                    "via": via(c), "batch": true, "batch_id": batch_id, "agent_id": agent_id,
                    "parked": stored.is_some_and(|r| r.parked),
                    "resume_expires_at": stored.and_then(|r| r.resume_expires_at),
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
    // (P4-GRP-1, D-130) An `access` change concerning the caller ends the long-poll at once with
    // what the caller may still decide (nothing, once they lost access): the client re-polls
    // and is re-authorized.
    let mut access = s.session_cache.subscribe_access();
    loop {
        let next = tokio::select! {
            r = tokio::time::timeout_at(deadline, rx.recv()) => r,
            p = access.recv() => {
                let relevant = match p {
                    Ok(p) => p.concerns(c.user_id),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => true,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => false,
                };
                if relevant {
                    return Ok(read().await?.into_iter().map(Into::into).collect());
                }
                continue;
            }
        };
        match next {
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
        parked: a.parked,
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
