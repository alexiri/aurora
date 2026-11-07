//! Secrets metadata (values live in the vault; `value_ref` is a SecretRef string).

use super::{After, split};
use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SecretRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub kind: String,
    pub llm_provider: Option<String>,
    pub llm_auth_mode: Option<String>,
    pub llm_endpoint_id: Option<String>,
    pub host_pattern: String,
    pub path_pattern: Option<String>,
    pub injection: serde_json::Value,
    pub value_source: String,
    pub value_ref: String,
    pub status: String,
    pub origin: Option<serde_json::Value>,
    pub value_version: i32,
    pub proposed_by_agent_id: Option<Uuid>,
    pub created_by: Option<Uuid>,
    pub created_by_name: Option<String>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const SELECT: &str = "SELECT s.id, s.org_id, s.workspace_id, s.name, s.kind, s.llm_provider, \
    s.llm_auth_mode, s.llm_endpoint_id, s.host_pattern, s.path_pattern, s.injection, \
    s.value_source, s.value_ref, s.status, s.origin, s.value_version, s.proposed_by_agent_id, \
    s.created_by, u.display_name AS created_by_name, s.last_used_at, s.created_at, s.updated_at \
    FROM secrets s LEFT JOIN users u ON u.id = s.created_by";

pub const NAME_CONSTRAINT: &str = "secrets_scope_name_uq";

pub struct NewSecret<'a> {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub name: &'a str,
    pub kind: &'a str,
    pub llm_provider: Option<&'a str>,
    pub llm_auth_mode: Option<&'a str>,
    pub llm_endpoint_id: Option<&'a str>,
    pub host_pattern: &'a str,
    pub path_pattern: Option<&'a str>,
    pub injection: &'a serde_json::Value,
    pub value_source: &'a str,
    pub value_ref: &'a str,
    pub status: &'a str,
    pub origin: Option<&'a serde_json::Value>,
    pub proposed_by_agent_id: Option<Uuid>,
    pub created_by: Option<Uuid>,
}

pub async fn insert<'e, E: PgExecutor<'e>>(exec: E, n: &NewSecret<'_>) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO secrets (id, org_id, workspace_id, name, kind, llm_provider, llm_auth_mode, \
         llm_endpoint_id, host_pattern, path_pattern, injection, value_source, value_ref, status, \
         origin, proposed_by_agent_id, created_by) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)",
    )
    .bind(n.id)
    .bind(n.org_id)
    .bind(n.workspace_id)
    .bind(n.name)
    .bind(n.kind)
    .bind(n.llm_provider)
    .bind(n.llm_auth_mode)
    .bind(n.llm_endpoint_id)
    .bind(n.host_pattern)
    .bind(n.path_pattern)
    .bind(n.injection)
    .bind(n.value_source)
    .bind(n.value_ref)
    .bind(n.status)
    .bind(n.origin)
    .bind(n.proposed_by_agent_id)
    .bind(n.created_by)
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn get<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<SecretRow>, DbError> {
    Ok(sqlx::query_as(&format!("{SELECT} WHERE s.id = $1"))
        .bind(id)
        .fetch_optional(exec)
        .await?)
}

/// Secrets of one scope (`workspace = None` = organization scope). Without a status
/// filter, rejected proposals are hidden.
pub async fn list_scope<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    workspace: Option<Uuid>,
    kind: Option<&str>,
    status: Option<&str>,
    after: After,
    fetch: i64,
) -> Result<Vec<SecretRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(&format!(
        "{SELECT} WHERE s.org_id = $1 AND s.workspace_id IS NOT DISTINCT FROM $2 \
         AND ($3::text IS NULL OR s.kind = $3) \
         AND (($4::text IS NULL AND s.status <> 'rejected') OR s.status = $4) \
         AND ($5::timestamptz IS NULL OR (s.created_at, s.id) < ($5, $6)) \
         ORDER BY s.created_at DESC, s.id DESC LIMIT $7"
    ))
    .bind(org)
    .bind(workspace)
    .bind(kind)
    .bind(status)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

pub async fn update_meta<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    name: Option<&str>,
    host_pattern: Option<&str>,
    path_pattern: Option<Option<&str>>,
    injection: Option<&serde_json::Value>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE secrets SET name = COALESCE($2, name), host_pattern = COALESCE($3, host_pattern), \
         path_pattern = CASE WHEN $4 THEN $5 ELSE path_pattern END, \
         injection = COALESCE($6, injection), updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(name)
    .bind(host_pattern)
    .bind(path_pattern.is_some())
    .bind(path_pattern.flatten())
    .bind(injection)
    .execute(exec)
    .await?;
    Ok(())
}

/// Compare-and-set value swap; bumps `value_version`. `false` = lost the race.
pub async fn replace_value<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    expected_ref: &str,
    expected_version: i32,
    new_ref: &str,
    source: &str,
    origin: Option<&serde_json::Value>,
) -> Result<bool, DbError> {
    let r = sqlx::query(
        "UPDATE secrets SET value_ref = $4, value_source = $5, value_version = value_version + 1, \
         origin = $6, updated_at = now() \
         WHERE id = $1 AND value_ref = $2 AND value_version = $3 AND status = 'active'",
    )
    .bind(id)
    .bind(expected_ref)
    .bind(expected_version)
    .bind(new_ref)
    .bind(source)
    .bind(origin)
    .execute(exec)
    .await?;
    Ok(r.rows_affected() == 1)
}

pub async fn set_origin<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    origin: &serde_json::Value,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE secrets SET origin = $2, updated_at = now() WHERE id = $1 AND kind = 'llm'",
    )
    .bind(id)
    .bind(origin)
    .execute(exec)
    .await?;
    Ok(())
}

/// Proposal -> active with its value. `false` = not (or no longer) proposed.
pub async fn activate<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    value_ref: &str,
    source: &str,
) -> Result<bool, DbError> {
    let r = sqlx::query(
        "UPDATE secrets SET value_ref = $2, value_source = $3, status = 'active', updated_at = now() \
         WHERE id = $1 AND status = 'proposed'",
    )
    .bind(id)
    .bind(value_ref)
    .bind(source)
    .execute(exec)
    .await?;
    Ok(r.rows_affected() == 1)
}

pub async fn reject<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<bool, DbError> {
    let r = sqlx::query(
        "UPDATE secrets SET status = 'rejected', updated_at = now() WHERE id = $1 AND status = 'proposed'",
    )
    .bind(id)
    .execute(exec)
    .await?;
    Ok(r.rows_affected() == 1)
}

pub async fn delete<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<(), DbError> {
    sqlx::query("DELETE FROM secrets WHERE id = $1")
        .bind(id)
        .execute(exec)
        .await?;
    Ok(())
}
