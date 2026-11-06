//! Conversations (docs/03 §6, docs/10 §Conversations).

use super::{After, split};
use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ConversationRow {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub workspace_id: Uuid,
    pub kind: String,
    pub external_ref: String,
    pub direct_user_id: Option<Uuid>,
    pub title: Option<String>,
    pub origin_conversation_id: Option<Uuid>,
    pub last_seq: i64,
    pub last_activity_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

const COLS: &str = "id, agent_id, workspace_id, kind, external_ref, direct_user_id, title, \
    origin_conversation_id, last_seq, last_activity_at, created_at";

pub struct NewConversation<'a> {
    pub agent_id: Uuid,
    pub workspace_id: Uuid,
    pub kind: &'a str,
    pub external_ref: &'a str,
    pub direct_user_id: Option<Uuid>,
    pub title: Option<&'a str>,
    pub origin_conversation_id: Option<Uuid>,
}

pub async fn get<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<ConversationRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("SELECT {COLS} FROM conversations WHERE id = $1"))
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}

/// Get-or-create by `(agent, kind, external_ref)`; idempotent under concurrency (`ON CONFLICT
/// DO NOTHING` without a target absorbs both the ref and the direct-user unique indexes).
/// Returns `(row, created)`.
pub async fn get_or_create(
    conn: &mut PgConnection,
    n: &NewConversation<'_>,
) -> Result<(ConversationRow, bool), DbError> {
    let done = sqlx::query(
        "INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, direct_user_id,
            title, origin_conversation_id, last_activity_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8, now()) ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(n.agent_id)
    .bind(n.workspace_id)
    .bind(n.kind)
    .bind(n.external_ref)
    .bind(n.direct_user_id)
    .bind(n.title)
    .bind(n.origin_conversation_id)
    .execute(&mut *conn)
    .await?;
    let row = sqlx::query_as(&format!(
        "SELECT {COLS} FROM conversations WHERE agent_id = $1 AND kind = $2 AND external_ref = $3"
    ))
    .bind(n.agent_id)
    .bind(n.kind)
    .bind(n.external_ref)
    .fetch_one(&mut *conn)
    .await?;
    Ok((row, done.rows_affected() == 1))
}

/// Conversations of an agent by last activity (desc); `after` is `(last_activity_at, id)`.
/// Direct conversations of other users are excluded (`viewer`).
pub async fn list_for_agent<'e, E: PgExecutor<'e>>(
    exec: E,
    agent_id: Uuid,
    kind: Option<&str>,
    viewer: Uuid,
    after: After,
    fetch: i64,
) -> Result<Vec<ConversationRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM conversations
         WHERE agent_id = $1 AND ($2::text IS NULL OR kind = $2)
           AND (kind <> 'direct' OR direct_user_id = $3)
           AND ($4::timestamptz IS NULL OR (last_activity_at, id) < ($4, $5))
         ORDER BY last_activity_at DESC, id DESC LIMIT $6"
    ))
    .bind(agent_id)
    .bind(kind)
    .bind(viewer)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

/// Take the conversation row lock (lock order: conversation, then its turns). Returns false when
/// the conversation does not exist.
pub async fn lock(conn: &mut PgConnection, id: Uuid) -> Result<bool, DbError> {
    let r: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM conversations WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    Ok(r.is_some())
}
