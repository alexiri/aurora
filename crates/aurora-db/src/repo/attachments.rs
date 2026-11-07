//! Attachments (docs/03 §6, docs/10 §Attachments): inbound uploads (`pending` until bound to a
//! turn) and outbound files (`bound` from birth). Binding lives in `turns::bind_attachments`.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AttachmentRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub conversation_id: Uuid,
    pub turn_id: Option<Uuid>,
    pub direction: String,
    pub status: String,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub blob_backend: String,
    pub blob_key: String,
    pub caption: Option<String>,
    pub created_by_user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

const COLS: &str = "id, workspace_id, conversation_id, turn_id, direction, status, filename, \
    content_type, size_bytes, sha256, blob_backend, blob_key, caption, created_by_user_id, created_at";

pub struct NewAttachment<'a> {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub conversation_id: Uuid,
    pub turn_id: Option<Uuid>,
    /// `in` | `out`
    pub direction: &'a str,
    /// `pending` | `bound`
    pub status: &'a str,
    pub filename: &'a str,
    pub content_type: &'a str,
    pub size_bytes: i64,
    pub sha256: &'a str,
    pub blob_backend: &'a str,
    pub blob_key: &'a str,
    pub caption: Option<&'a str>,
    pub created_by_user_id: Option<Uuid>,
}

pub async fn insert(
    exec: impl PgExecutor<'_>,
    n: &NewAttachment<'_>,
) -> Result<AttachmentRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO attachments (id, workspace_id, conversation_id, turn_id, direction, status,
            filename, content_type, size_bytes, sha256, blob_backend, blob_key, caption,
            created_by_user_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
         RETURNING {COLS}"
    ))
    .bind(n.id)
    .bind(n.workspace_id)
    .bind(n.conversation_id)
    .bind(n.turn_id)
    .bind(n.direction)
    .bind(n.status)
    .bind(n.filename)
    .bind(n.content_type)
    .bind(n.size_bytes)
    .bind(n.sha256)
    .bind(n.blob_backend)
    .bind(n.blob_key)
    .bind(n.caption)
    .bind(n.created_by_user_id)
    .fetch_one(exec)
    .await?)
}

pub async fn get(exec: impl PgExecutor<'_>, id: Uuid) -> Result<Option<AttachmentRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("SELECT {COLS} FROM attachments WHERE id = $1"))
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}

/// Inbound attachment readable by `runner`: `bound` to a turn that is in flight (`dispatched` /
/// `running`) on the current boot of a sandbox this runner owns (docs/16 §W5).
pub async fn get_for_runner(
    exec: impl PgExecutor<'_>,
    id: Uuid,
    runner: Uuid,
) -> Result<Option<AttachmentRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT a.id, a.workspace_id, a.conversation_id, a.turn_id, a.direction, a.status,
                a.filename, a.content_type, a.size_bytes, a.sha256, a.blob_backend, a.blob_key,
                a.caption, a.created_by_user_id, a.created_at
         FROM attachments a
         JOIN turns t ON t.id = a.turn_id AND t.role = 'turn'
         JOIN sandboxes s ON s.agent_id = t.agent_id
         WHERE a.id = $1 AND a.direction = 'in' AND a.status = 'bound'
           AND t.status IN ('dispatched', 'running')
           AND t.boot_id IS NOT NULL AND s.boot_id = t.boot_id AND s.runner_id = $2",
    )
    .bind(id)
    .bind(runner)
    .fetch_optional(exec)
    .await?)
}

/// `(outbound files already stored for the turn, outbound bytes of the conversation in the last
/// 24 h)`. Call under the conversation lock so concurrent uploads cannot overshoot the caps.
pub async fn out_usage(
    conn: &mut PgConnection,
    conversation_id: Uuid,
    turn_id: Uuid,
) -> Result<(i64, i64), DbError> {
    Ok(sqlx::query_as(
        "SELECT count(*) FILTER (WHERE turn_id = $2)::bigint,
                COALESCE(sum(size_bytes) FILTER (WHERE created_at > now() - interval '24 hours'), 0)::bigint
         FROM attachments WHERE conversation_id = $1 AND direction = 'out'",
    )
    .bind(conversation_id)
    .bind(turn_id)
    .fetch_one(conn)
    .await?)
}

/// Delete up to `limit` `pending` rows older than `ttl_secs` (row-claiming, so a concurrent bind
/// loses cleanly) and return their `(blob_backend, blob_key)` for blob removal.
pub async fn delete_expired_pending(
    exec: impl PgExecutor<'_>,
    ttl_secs: f64,
    limit: i64,
) -> Result<Vec<(String, String)>, DbError> {
    Ok(sqlx::query_as(
        "DELETE FROM attachments WHERE id IN (
             SELECT id FROM attachments
             WHERE status = 'pending' AND direction = 'in'
               AND created_at < now() - make_interval(secs => $1)
             ORDER BY created_at LIMIT $2 FOR UPDATE SKIP LOCKED)
         RETURNING blob_backend, blob_key",
    )
    .bind(ttl_secs)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// A fetch that failed (file too large, connector error): a `failed` row bound to the turn so
/// the transcript shows what was dropped. It carries no blob.
#[allow(clippy::too_many_arguments)]
pub async fn insert_failed(
    exec: impl PgExecutor<'_>,
    id: Uuid,
    workspace_id: Uuid,
    conversation_id: Uuid,
    turn_id: Uuid,
    filename: &str,
    content_type: &str,
    size_bytes: i64,
    reason: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO attachments (id, workspace_id, conversation_id, turn_id, direction, status,
            filename, content_type, size_bytes, sha256, blob_backend, blob_key, failure_reason)
         VALUES ($1,$2,$3,$4,'in','failed',$5,$6,$7,'','','',$8)",
    )
    .bind(id)
    .bind(workspace_id)
    .bind(conversation_id)
    .bind(turn_id)
    .bind(filename)
    .bind(content_type)
    .bind(size_bytes)
    .bind(reason)
    .execute(exec)
    .await?;
    Ok(())
}

/// Bound outbound files of a turn (`send_file`), oldest first.
pub async fn out_for_turn(
    exec: impl PgExecutor<'_>,
    turn_id: Uuid,
) -> Result<Vec<AttachmentRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM attachments
          WHERE turn_id=$1 AND direction='out' AND status='bound' ORDER BY created_at, id"
    ))
    .bind(turn_id)
    .fetch_all(exec)
    .await?)
}
