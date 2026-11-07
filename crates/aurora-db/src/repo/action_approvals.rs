//! Platform-level action approvals (docs/07 §Action approvals). Separate from the gateway
//! `approvals` table. Every state change is fenced on the expected current status.

use super::{After, split};
use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ActionApprovalRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub action_key: String,
    pub payload: serde_json::Value,
    pub summary: String,
    pub status: String,
    pub decided_by_user_id: Option<Uuid>,
    pub decided_by_name: Option<String>,
    pub decided_at: Option<DateTime<Utc>>,
    pub executed_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    /// Workspace of `conversation_id`'s conversation (null without one).
    pub conversation_workspace_id: Option<Uuid>,
}

const SELECT: &str = "SELECT a.id, a.workspace_id, a.agent_id, a.conversation_id, a.turn_id, \
    a.action_key, a.payload, a.summary, a.status, a.decided_by_user_id, \
    u.display_name AS decided_by_name, a.decided_at, a.executed_at, a.error, a.expires_at, \
    a.created_at, cv.workspace_id AS conversation_workspace_id FROM action_approvals a \
    LEFT JOIN users u ON u.id = a.decided_by_user_id \
    LEFT JOIN conversations cv ON cv.id = a.conversation_id";

pub struct NewActionApproval<'a> {
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub action_key: &'a str,
    pub payload: &'a serde_json::Value,
    pub summary: &'a str,
    pub expires_in_secs: f64,
}

pub async fn insert(
    conn: &mut PgConnection,
    n: &NewActionApproval<'_>,
) -> Result<ActionApprovalRow, DbError> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO action_approvals (id, workspace_id, agent_id, conversation_id, turn_id,
            action_key, payload, summary, status, expires_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'pending', now() + make_interval(secs => $9))",
    )
    .bind(id)
    .bind(n.workspace_id)
    .bind(n.agent_id)
    .bind(n.conversation_id)
    .bind(n.turn_id)
    .bind(n.action_key)
    .bind(n.payload)
    .bind(n.summary)
    .bind(n.expires_in_secs)
    .execute(&mut *conn)
    .await?;
    get(&mut *conn, id).await?.ok_or(DbError::NotFound)
}

pub async fn get<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<ActionApprovalRow>, DbError> {
    Ok(sqlx::query_as(&format!("{SELECT} WHERE a.id = $1"))
        .bind(id)
        .fetch_optional(exec)
        .await?)
}

/// Row-locked read (transaction required).
pub async fn get_for_update(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<ActionApprovalRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("{SELECT} WHERE a.id = $1 FOR UPDATE OF a"))
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

/// Approvals of a workspace, newest first; `after` is `(created_at, id)`.
pub async fn list_workspace<'e, E: PgExecutor<'e>>(
    exec: E,
    viewer: Uuid,
    workspace_id: Uuid,
    status: Option<&str>,
    agent_id: Option<Uuid>,
    after: After,
    fetch: i64,
) -> Result<Vec<ActionApprovalRow>, DbError> {
    let (t, id) = split(after);
    // RFE-009: rows naming an invisible direct conversation are omitted.
    Ok(sqlx::query_as(&format!(
        "{SELECT} WHERE a.workspace_id = $1 AND ($2::text IS NULL OR a.status = $2)
           AND ($3::uuid IS NULL OR a.agent_id = $3)
           AND ($4::timestamptz IS NULL OR (a.created_at, a.id) < ($4, $5))
           AND NOT {}
         ORDER BY a.created_at DESC, a.id DESC LIMIT $6",
        crate::authz::hidden_direct_conversation_sql("a.conversation_id", "$7", "a.workspace_id")
    ))
    .bind(workspace_id)
    .bind(status)
    .bind(agent_id)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .bind(viewer)
    .fetch_all(exec)
    .await?)
}

/// `pending` -> `approved` | `rejected` (fenced). `None` when the row is no longer pending.
pub async fn decide(
    conn: &mut PgConnection,
    id: Uuid,
    approve: bool,
    user: Uuid,
) -> Result<Option<ActionApprovalRow>, DbError> {
    let done = sqlx::query(
        "UPDATE action_approvals SET status = $2, decided_by_user_id = $3, decided_at = now(),
            updated_at = now()
         WHERE id = $1 AND status = 'pending'",
    )
    .bind(id)
    .bind(if approve { "approved" } else { "rejected" })
    .bind(user)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(None);
    }
    get(conn, id).await
}

/// `approved` -> `executed` (fenced).
pub async fn mark_executed(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<ActionApprovalRow>, DbError> {
    let done = sqlx::query(
        "UPDATE action_approvals SET status = 'executed', executed_at = now(), updated_at = now()
         WHERE id = $1 AND status = 'approved'",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(None);
    }
    get(conn, id).await
}

/// `approved` -> `failed` with a bounded error (fenced).
pub async fn mark_failed(
    conn: &mut PgConnection,
    id: Uuid,
    error: &str,
) -> Result<Option<ActionApprovalRow>, DbError> {
    let done = sqlx::query(
        "UPDATE action_approvals SET status = 'failed', executed_at = now(), error = $2,
            updated_at = now()
         WHERE id = $1 AND status = 'approved'",
    )
    .bind(id)
    .bind(error)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(None);
    }
    get(conn, id).await
}

/// `pending` -> `expired` (fenced).
pub async fn expire(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<ActionApprovalRow>, DbError> {
    let done = sqlx::query(
        "UPDATE action_approvals SET status = 'expired', updated_at = now()
         WHERE id = $1 AND status = 'pending'",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(None);
    }
    get(conn, id).await
}

/// Ids of `pending` rows past `expires_at` (oldest first).
pub async fn overdue_ids<'e, E: PgExecutor<'e>>(exec: E, limit: i64) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM action_approvals WHERE status = 'pending' AND expires_at <= now()
         ORDER BY expires_at LIMIT $1",
    )
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// A pending `message_agent` approval for this sender, link and side exists.
pub async fn pending_message_agent_exists<'e, E: PgExecutor<'e>>(
    exec: E,
    agent_id: Uuid,
    link_id: Uuid,
    side_agent_id: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM action_approvals
           WHERE action_key = 'message_agent' AND status = 'pending' AND agent_id = $1
             AND payload->>'link_id' = $2 AND payload->>'side_agent_id' = $3)",
    )
    .bind(agent_id)
    .bind(link_id.to_string())
    .bind(side_agent_id.to_string())
    .fetch_one(exec)
    .await?)
}
