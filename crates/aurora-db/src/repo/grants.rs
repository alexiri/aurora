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

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ConnectionGrantRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub connection_id: Uuid,
    pub mode: String,
    pub allow_tools: Vec<String>,
    pub ask_tools: Vec<String>,
    /// (RFE-011a) hold deadline preference of the ask rule, seconds.
    pub ask_timeout_secs: Option<i32>,
    pub created_at: DateTime<Utc>,
}

const CONN_COLS: &str = "id, workspace_id, agent_id, connection_id, mode, allow_tools, ask_tools, \
     ask_timeout_secs, created_at";

/// Create or replace the `(agent, connection)` grant (mode and tool lists are overwritten).
#[allow(clippy::too_many_arguments)]
pub async fn upsert_connection_grant<'e, E: PgExecutor<'e>>(
    exec: E,
    workspace_id: Uuid,
    agent_id: Uuid,
    connection_id: Uuid,
    mode: &str,
    allow_tools: &[String],
    ask_tools: &[String],
    ask_timeout_secs: Option<i32>,
    created_by: Option<Uuid>,
) -> Result<ConnectionGrantRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO grants (id, workspace_id, agent_id, connection_id, mode, allow_tools, ask_tools, \
         created_by, ask_timeout_secs) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) \
         ON CONFLICT (agent_id, connection_id) DO UPDATE SET mode = EXCLUDED.mode, \
         allow_tools = EXCLUDED.allow_tools, ask_tools = EXCLUDED.ask_tools, \
         ask_timeout_secs = EXCLUDED.ask_timeout_secs, updated_at = now() \
         RETURNING {CONN_COLS}"
    ))
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(agent_id)
    .bind(connection_id)
    .bind(mode)
    .bind(allow_tools)
    .bind(ask_tools)
    .bind(created_by)
    .bind(ask_timeout_secs)
    .fetch_one(exec)
    .await?)
}

/// Connection grants of one agent, oldest first.
pub async fn list_connection_grants<'e, E: PgExecutor<'e>>(
    exec: E,
    agent_id: Uuid,
) -> Result<Vec<ConnectionGrantRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {CONN_COLS} FROM grants WHERE agent_id = $1 AND connection_id IS NOT NULL \
         ORDER BY created_at, id"
    ))
    .bind(agent_id)
    .fetch_all(exec)
    .await?)
}

/// Grants on one connection, oldest first.
pub async fn list_for_connection<'e, E: PgExecutor<'e>>(
    exec: E,
    connection_id: Uuid,
) -> Result<Vec<ConnectionGrantRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {CONN_COLS} FROM grants WHERE connection_id = $1 ORDER BY created_at, id"
    ))
    .bind(connection_id)
    .fetch_all(exec)
    .await?)
}

/// Delete the `(agent, connection)` grant; returns its id when it existed.
pub async fn delete_connection_grant<'e, E: PgExecutor<'e>>(
    exec: E,
    agent_id: Uuid,
    connection_id: Uuid,
) -> Result<Option<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "DELETE FROM grants WHERE agent_id = $1 AND connection_id = $2 RETURNING id",
    )
    .bind(agent_id)
    .bind(connection_id)
    .fetch_optional(exec)
    .await?)
}
