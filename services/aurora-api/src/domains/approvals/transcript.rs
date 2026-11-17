//! (P5-UX-16, C-983, `10 §Events`, C-825) The transcript events of an approval that belongs to a
//! conversation: `approval.pending` when it is created, `approval.decided` when it is answered,
//! `approval.expired` when it ran out (`reason: "deadline"`, or `"resume_window"` for an approved
//! parked approval nobody repeated).
//!
//! Every writer goes through [`write`]: under the conversation's row lock it appends the event
//! only if the conversation has none of that type for that approval yet, so the decision
//! transaction, the bus listener and the sweeps can all call it without duplicating a card.
//! Payloads hold sanitized one-line facts only (never request fields or bodies).

use super::dto::{AlwaysApplied, Approval};
use super::enrich::enrich;
use crate::app::AppState;
use crate::domains::conversations::service as conv;
use aurora_core::{ApiError, sanitize};
use aurora_db::DbError;
use aurora_db::repo::approvals::ApprovalRow;
use aurora_db::repo::conversations as convs;
use serde_json::json;
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Pending,
    Decided,
    /// `reason`: `deadline` | `resume_window`.
    Expired(&'static str),
}

impl Kind {
    fn ty(self) -> &'static str {
        match self {
            Self::Pending => "approval.pending",
            Self::Decided => "approval.decided",
            Self::Expired(_) => "approval.expired",
        }
    }
}

fn line(s: &str) -> String {
    sanitize::one_line(s, 120)
}

/// The sentence as one line: "<Agent> wants to <action> in <App>".
fn sentence_text(a: &Approval) -> String {
    let mut out = format!("{} wants to {}", a.sentence.agent.name, a.sentence.action);
    if let Some(app) = &a.sentence.app {
        out.push_str(" in ");
        out.push_str(&app.name);
    }
    line(&out)
}

/// Appends the event unless the conversation already has it for this approval. Does nothing for an
/// approval without a conversation or whose conversation is gone.
pub async fn write(
    s: &AppState,
    tx: &mut PgConnection,
    row: &ApprovalRow,
    kind: Kind,
    always: Option<&AlwaysApplied>,
) -> Result<(), ApiError> {
    let Some(cid) = row.conversation_id else {
        return Ok(());
    };
    if !convs::lock(&mut *tx, cid).await? {
        return Ok(());
    }
    let ty = kind.ty();
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM transcript_events WHERE conversation_id = $1 \
         AND type = $2 AND payload->>'approval_id' = $3)",
    )
    .bind(cid)
    .bind(ty)
    .bind(row.id.to_string())
    .fetch_one(&mut *tx)
    .await
    .map_err(DbError::from)?;
    // `approval.expired` with `resume_window` follows a `decided`; both reasons are one event each.
    if exists {
        return Ok(());
    }
    let mut a = Approval::from(row.clone());
    enrich(s, None, std::slice::from_mut(&mut a)).await;
    let mut payload = json!({
        "approval_id": row.id,
        "summary": sentence_text(&a),
        "agent_name": a.sentence.agent.name,
        "action": line(&a.sentence.action),
    });
    if let Some(app) = &a.sentence.app {
        payload["app_name"] = json!(app.name);
    }
    if let Some(agent) = &a.agent {
        payload["agent_handle"] = json!(agent.handle);
    }
    match kind {
        Kind::Pending => {
            payload["expires_at"] = json!(row.deadline);
            payload["parked"] = json!(row.parked);
        }
        Kind::Decided => {
            payload["decision"] = json!(if row.status == "approved" {
                "approve"
            } else {
                "deny"
            });
            payload["via"] = json!(row.decided_via);
            if let Some(name) = &row.decided_by_name {
                payload["decided_by_label"] = json!(line(name));
            }
            if row.parked
                && let Some(t) = row.resume_expires_at
            {
                payload["resume_expires_at"] = json!(t);
            }
            if let Some(al) = always {
                payload["always"] = json!({
                    "tool": { "id": al.tool.id, "name": line(&al.tool.name) },
                    "also_approved": al.also_approved,
                });
            }
        }
        Kind::Expired(reason) => payload["reason"] = json!(reason),
    }
    conv::append_event(tx, cid, row.turn_id, ty, &payload).await?;
    Ok(())
}

/// The event a row's current state calls for, if any (what the bus listener writes for a change it
/// was told about): pending → `approval.pending`; approved or denied → `approval.decided`;
/// expired → `approval.expired`.
pub fn kind_of(row: &ApprovalRow) -> Option<Kind> {
    match row.status.as_str() {
        "pending" => Some(Kind::Pending),
        "approved" | "denied" => Some(Kind::Decided),
        "expired" => Some(Kind::Expired("deadline")),
        _ => None,
    }
}

/// Writes what the stored row calls for (idempotent); used by the bus listener.
pub async fn write_current(s: &AppState, id: uuid::Uuid) -> Result<(), ApiError> {
    let Some(row) = aurora_db::repo::approvals::get(&s.pool, id).await? else {
        return Ok(());
    };
    if row.conversation_id.is_none() {
        return Ok(());
    }
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    // A decided row is preceded by its `approval.pending` when the listener saw only the decision.
    if let Some(kind) = kind_of(&row) {
        if kind != Kind::Pending {
            write(s, &mut tx, &row, Kind::Pending, None).await?;
        }
        write(s, &mut tx, &row, kind, None).await?;
    }
    // An approved parked approval nobody repeated in time: the sweep ended its window and notified
    // `expired` (C-828); the card then says the approval ran out (`resume_window`).
    if row.status == "approved" && row.resume_state.as_deref() == Some("expired") {
        write(s, &mut tx, &row, Kind::Expired("resume_window"), None).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// The listener: every bus event (`created`, `decided`, `expired`, `abandoned`) of an approval
/// with a conversation gets its transcript event. Runs once per API process; the per-conversation
/// lock and the existence check make several replicas harmless.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        loop {
            let mut rx = match state.approval_bus.subscribe(&state.pool).await {
                Ok(rx) => rx,
                Err(e) => {
                    tracing::warn!(error = %e.message(), "approval transcript listener failed to subscribe");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                }
            };
            loop {
                match rx.recv().await {
                    Ok(super::bus::BusMsg::Event { id, .. }) => {
                        if let Err(e) = write_current(&state, id).await {
                            tracing::warn!(error = %e.message(), approval_id = %id, "writing an approval transcript event failed");
                        }
                    }
                    Ok(super::bus::BusMsg::Resync)
                    | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    });
}
