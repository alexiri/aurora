//! Agent grants (secret grants; connection grants arrive with M3).

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SecretGrantRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub secret_id: Uuid,
    pub created_at: DateTime<Utc>,
}

/// Idempotent: returns the existing grant when `(agent, secret)` is already granted.
pub async fn upsert_secret_grant<'e, E: PgExecutor<'e>>(
    exec: E,
    workspace_id: Uuid,
    agent_id: Uuid,
    secret_id: Uuid,
    created_by: Option<Uuid>,
) -> Result<SecretGrantRow, DbError> {
    Ok(sqlx::query_as(
        "INSERT INTO grants (id, workspace_id, agent_id, secret_id, created_by) \
         VALUES ($1,$2,$3,$4,$5) \
         ON CONFLICT (agent_id, secret_id) DO UPDATE SET updated_at = now() \
         RETURNING id, workspace_id, agent_id, secret_id, created_at",
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(agent_id)
    .bind(secret_id)
    .bind(created_by)
    .fetch_one(exec)
    .await?)
}
