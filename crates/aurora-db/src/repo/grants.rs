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

/// Secret grants of one agent, oldest first.
pub async fn list_secret_grants<'e, E: PgExecutor<'e>>(
    exec: E,
    agent_id: Uuid,
) -> Result<Vec<SecretGrantRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, workspace_id, agent_id, secret_id, created_at FROM grants \
         WHERE agent_id = $1 AND secret_id IS NOT NULL ORDER BY created_at, id",
    )
    .bind(agent_id)
    .fetch_all(exec)
    .await?)
}

/// Delete the `(agent, secret)` grant; returns its id when it existed.
pub async fn delete_secret_grant<'e, E: PgExecutor<'e>>(
    exec: E,
    agent_id: Uuid,
    secret_id: Uuid,
) -> Result<Option<Uuid>, DbError> {
    Ok(
        sqlx::query_scalar(
            "DELETE FROM grants WHERE agent_id = $1 AND secret_id = $2 RETURNING id",
        )
        .bind(agent_id)
        .bind(secret_id)
        .fetch_optional(exec)
        .await?,
    )
}
