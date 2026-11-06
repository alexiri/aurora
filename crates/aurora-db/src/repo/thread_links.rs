//! Platform thread <-> conversation links (docs/13 §5.1; schema `thread_links`).

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ThreadLinkRow {
    pub id: Uuid,
    pub integration_id: Uuid,
    pub agent_id: Uuid,
    pub conversation_id: Uuid,
    pub kind: String,
    pub space_ref: String,
    pub thread_ref: String,
    pub sender: serde_json::Value,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
}

const COLS: &str = "id, integration_id, agent_id, conversation_id, kind, space_ref, thread_ref, sender, revision, created_at";

/// The link of a platform thread (`thread_ref` is `''` on platforms without threads).
pub async fn thread_link<'e, E: PgExecutor<'e>>(
    exec: E,
    integration: Uuid,
    agent: Uuid,
    space_ref: &str,
    thread_ref: &str,
) -> Result<Option<ThreadLinkRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM thread_links
          WHERE integration_id=$1 AND agent_id=$2 AND space_ref=$3 AND thread_ref=$4"
    ))
    .bind(integration)
    .bind(agent)
    .bind(space_ref)
    .bind(thread_ref)
    .fetch_optional(exec)
    .await?)
}

pub async fn for_conversation<'e, E: PgExecutor<'e>>(
    exec: E,
    conversation: Uuid,
) -> Result<Option<ThreadLinkRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM thread_links WHERE conversation_id=$1 ORDER BY created_at LIMIT 1"
    ))
    .bind(conversation)
    .fetch_optional(exec)
    .await?)
}

pub struct UpsertThreadLink<'a> {
    pub integration_id: Uuid,
    pub agent_id: Uuid,
    pub conversation_id: Uuid,
    pub kind: &'a str,
    pub space_ref: &'a str,
    pub thread_ref: &'a str,
    pub sender: &'a serde_json::Value,
}

/// Create the link, or return the existing one for the same thread untouched (first writer
/// wins, so concurrent inbox workers converge on one conversation).
pub async fn upsert_thread_link<'e, E: PgExecutor<'e>>(
    exec: E,
    n: &UpsertThreadLink<'_>,
) -> Result<ThreadLinkRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO thread_links (id, integration_id, agent_id, conversation_id, kind, space_ref,
             thread_ref, sender)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
         ON CONFLICT (integration_id, agent_id, space_ref, thread_ref)
         DO UPDATE SET updated_at = thread_links.updated_at
         RETURNING {COLS}"
    ))
    .bind(Uuid::now_v7())
    .bind(n.integration_id)
    .bind(n.agent_id)
    .bind(n.conversation_id)
    .bind(n.kind)
    .bind(n.space_ref)
    .bind(n.thread_ref)
    .bind(n.sender)
    .fetch_one(exec)
    .await?)
}

/// Bump the link revision (mirror cursor); returns the new revision.
pub async fn bump_revision<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<i64>, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE thread_links SET revision=revision+1, updated_at=now() WHERE id=$1 RETURNING revision",
    )
    .bind(id)
    .fetch_optional(exec)
    .await?)
}
