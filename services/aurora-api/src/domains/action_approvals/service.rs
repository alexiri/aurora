//! Action-approval lifecycle: create, decide (`approved` is durable before the handler runs),
//! outcome notices and the 60 s expiry sweep.

use super::dto::{ActionApproval, ActionDecision, STATUSES};
use super::handlers::{self, ActionContext};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::conversations::service::{self as conv, NewMessage};
use crate::http::auth::Caller;
use aurora_core::{ApiError, FieldError, limits, sanitize};
use aurora_db::repo::action_approvals::{self as repo, ActionApprovalRow, NewActionApproval};
use aurora_db::repo::{conversations as convs, turns};
use aurora_db::{DbError, PgPool};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

/// Longest handler error kept on the row.
const ERROR_MAX: usize = 500;
/// Rows expired per sweep run.
const SWEEP_BATCH: i64 = 100;

pub fn validate_status(status: Option<&str>) -> Result<(), ApiError> {
    match status {
        Some(s) if !STATUSES.contains(&s) => Err(ApiError::invalid_request(
            "Invalid status.",
            vec![FieldError::new("/status", "unknown status")],
        )),
        _ => Ok(()),
    }
}

pub struct NewAction<'a> {
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub action_key: &'a str,
    /// Frozen arguments, executed verbatim after approval.
    pub payload: &'a Value,
    /// Platform-composed one-liner (never raw model text).
    pub summary: &'a str,
}

/// Insert a `pending` row expiring after `ACTION_APPROVAL_TTL`.
pub async fn create(
    conn: &mut PgConnection,
    n: NewAction<'_>,
) -> Result<ActionApprovalRow, ApiError> {
    if handlers::handler(n.action_key).is_none() {
        tracing::error!(
            key = n.action_key,
            "action approval for an unregistered key"
        );
        return Err(ApiError::internal("Unknown action."));
    }
    let summary = sanitize::one_line(n.summary, 300);
    Ok(repo::insert(
        conn,
        &NewActionApproval {
            workspace_id: n.workspace_id,
            agent_id: n.agent_id,
            conversation_id: n.conversation_id,
            turn_id: n.turn_id,
            action_key: n.action_key,
            payload: n.payload,
            summary: &summary,
            expires_in_secs: limits::ACTION_APPROVAL_TTL.as_secs_f64(),
        },
    )
    .await?)
}

fn already_decided() -> ApiError {
    ApiError::conflict(
        "already_decided",
        "This action approval has already been resolved.",
    )
}

/// Append the outcome to the origin conversation as a durable `notice` and, when asked and the
/// origin turn has finished, queue a system follow-up turn carrying the line (07 step 4).
async fn report_outcome(
    conn: &mut PgConnection,
    row: &ActionApprovalRow,
    code: &str,
    line: &str,
    notify_agent: bool,
) -> Result<(), ApiError> {
    let Some(cid) = row.conversation_id else {
        return Ok(());
    };
    if !convs::lock(&mut *conn, cid).await? {
        return Ok(());
    }
    conv::append_event(
        conn,
        cid,
        row.turn_id,
        "notice",
        &json!({"code": code, "text": line, "approval_id": row.id, "status": row.status}),
    )
    .await?;
    if !notify_agent {
        return Ok(());
    }
    let finished = match row.turn_id {
        None => true,
        Some(t) => turns::get(&mut *conn, t)
            .await?
            .is_none_or(|t| matches!(t.status.as_str(), "done" | "failed" | "aborted")),
    };
    if !finished {
        return Ok(());
    }
    let Some(c) = convs::get(&mut *conn, cid).await? else {
        return Ok(());
    };
    conv::create_turn_tx(
        conn,
        &c,
        &NewMessage {
            door: json!({"kind": "system"}),
            speaker_kind: "system",
            speaker_user_id: None,
            speaker_label: "Approval",
            text: line,
            source_event_id: None,
            attachment_ids: &[],
        },
    )
    .await?;
    Ok(())
}

fn audit_decision(s: &AppState, c: &Caller, org: Uuid, row: &ActionApprovalRow, decision: &str) {
    audit::record(
        s,
        AuditEvent::new(
            AuditActor::from_caller(c),
            "action_approval.decide",
            Outcome::Success,
        )
        .org(org)
        .workspace(row.workspace_id)
        .target("action_approval", row.id)
        .metadata(json!({"decision": decision, "action_key": row.action_key,
            "agent_id": row.agent_id, "status": row.status})),
    );
}

pub async fn decide(
    s: &AppState,
    c: &Caller,
    id: Uuid,
    decision: ActionDecision,
) -> Result<ActionApproval, ApiError> {
    let not_found = || ApiError::not_found("Action approval not found.");
    let existing = repo::get(&s.pool, id).await?.ok_or_else(not_found)?;
    // 404 when the workspace is invisible; key confinement applies.
    let auth = c.workspace(s, existing.workspace_id).await?;
    if !auth.can_use() {
        return Err(not_found());
    }
    // Fail loud (07): an unregistered key is a deployment defect, not a client error.
    let Some(handler) = handlers::handler(&existing.action_key) else {
        tracing::error!(approval = %id, key = %existing.action_key,
            "no handler registered for action approval");
        return Err(ApiError::internal("No handler for this action."));
    };

    // Step 1: record the decision durably.
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    let row = repo::get_for_update(&mut tx, id)
        .await?
        .ok_or_else(not_found)?;
    if row.status != "pending" {
        return Err(already_decided());
    }
    if row.expires_at <= chrono::Utc::now() {
        // Past its deadline: close it as expired (and tell the origin) rather than act on it.
        expire_one(&mut tx, &row).await?;
        tx.commit().await.map_err(DbError::from)?;
        return Err(already_decided());
    }
    let approve = decision == ActionDecision::Approve;
    let decided = repo::decide(&mut tx, id, approve, c.user_id)
        .await?
        .ok_or_else(already_decided)?;
    if !approve {
        let line = format!("The request \"{}\" was denied.", decided.summary);
        report_outcome(&mut tx, &decided, "action_approval_rejected", &line, true).await?;
        tx.commit().await.map_err(DbError::from)?;
        audit_decision(s, c, auth.org_id, &decided, "deny");
        return Ok(decided.into());
    }
    tx.commit().await.map_err(DbError::from)?;
    audit_decision(s, c, auth.org_id, &decided, "approve");

    // Step 2: execute. The row is durably `approved`, so a crash here stays visible.
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    let row = repo::get_for_update(&mut tx, id)
        .await?
        .ok_or_else(not_found)?;
    if row.status != "approved" {
        tx.commit().await.map_err(DbError::from)?;
        return Ok(row.into());
    }
    let result = {
        let mut sp = tx.begin().await.map_err(DbError::from)?;
        let ctx = ActionContext {
            approval: &row,
            decided_by: c.user_id,
        };
        match handler.execute(&mut sp, &ctx).await {
            Ok(out) => {
                sp.commit().await.map_err(DbError::from)?;
                Ok(out)
            }
            Err(e) => {
                sp.rollback().await.map_err(DbError::from)?;
                Err(e)
            }
        }
    };
    let done = match result {
        Ok(out) => {
            let done = repo::mark_executed(&mut tx, id)
                .await?
                .ok_or_else(already_decided)?;
            report_outcome(
                &mut tx,
                &done,
                "action_approval_executed",
                &out.line,
                out.notify_agent,
            )
            .await?;
            done
        }
        Err(e) => {
            let msg = sanitize::one_line(&e.0, ERROR_MAX);
            let done = repo::mark_failed(&mut tx, id, &msg)
                .await?
                .ok_or_else(already_decided)?;
            let line = format!(
                "The request \"{}\" was approved but could not be completed: {msg}",
                done.summary
            );
            report_outcome(&mut tx, &done, "action_approval_failed", &line, true).await?;
            done
        }
    };
    tx.commit().await.map_err(DbError::from)?;
    Ok(done.into())
}

/// Expire one locked pending row and tell the origin conversation.
async fn expire_one(conn: &mut PgConnection, row: &ActionApprovalRow) -> Result<bool, ApiError> {
    let Some(done) = repo::expire(&mut *conn, row.id).await? else {
        return Ok(false);
    };
    let line = format!(
        "The request \"{}\" expired without a decision.",
        done.summary
    );
    report_outcome(conn, &done, "action_approval_expired", &line, true).await?;
    Ok(true)
}

/// Expire pending rows past `expires_at` (every 60 s, idempotent). Returns how many expired.
pub async fn sweep(pool: &PgPool) -> Result<usize, ApiError> {
    let mut n = 0;
    for id in repo::overdue_ids(pool, SWEEP_BATCH).await? {
        let mut tx = pool.begin().await.map_err(DbError::from)?;
        if let Some(row) = repo::get_for_update(&mut tx, id).await?
            && row.status == "pending"
            && expire_one(&mut tx, &row).await?
        {
            n += 1;
        }
        tx.commit().await.map_err(DbError::from)?;
    }
    Ok(n)
}
