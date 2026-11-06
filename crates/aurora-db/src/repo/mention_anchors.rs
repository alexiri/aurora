//! Mention anchors: per-conversation alias -> platform subject (docs/13 §5.10; schema
//! `mention_anchors`). Aliases are stored normalized (lowercase, trimmed).

use crate::DbError;
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AnchorRow {
    pub conversation_id: Uuid,
    pub alias: String,
    pub integration_id: Uuid,
    pub kind: String,
    pub subject_ref: String,
    pub label: String,
}

pub fn normalize(alias: &str) -> String {
    alias.trim().trim_start_matches(['@', '#']).to_lowercase()
}

const COLS: &str = "conversation_id, alias, integration_id, kind, ref AS subject_ref, label";

pub async fn upsert<'e, E: PgExecutor<'e>>(
    exec: E,
    conversation: Uuid,
    alias: &str,
    integration: Uuid,
    kind: &str,
    subject_ref: &str,
    label: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO mention_anchors (conversation_id, alias, integration_id, kind, ref, label)
         VALUES ($1,$2,$3,$4,$5,$6)
         ON CONFLICT (conversation_id, alias)
         DO UPDATE SET integration_id=EXCLUDED.integration_id, kind=EXCLUDED.kind,
                       ref=EXCLUDED.ref, label=EXCLUDED.label, updated_at=now()",
    )
    .bind(conversation)
    .bind(normalize(alias))
    .bind(integration)
    .bind(kind)
    .bind(subject_ref)
    .bind(label)
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn resolve<'e, E: PgExecutor<'e>>(
    exec: E,
    conversation: Uuid,
    alias: &str,
) -> Result<Option<AnchorRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM mention_anchors WHERE conversation_id=$1 AND alias=$2"
    ))
    .bind(conversation)
    .bind(normalize(alias))
    .fetch_optional(exec)
    .await?)
}

pub async fn list_for_conversation<'e, E: PgExecutor<'e>>(
    exec: E,
    conversation: Uuid,
) -> Result<Vec<AnchorRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM mention_anchors WHERE conversation_id=$1 ORDER BY alias"
    ))
    .bind(conversation)
    .fetch_all(exec)
    .await?)
}
