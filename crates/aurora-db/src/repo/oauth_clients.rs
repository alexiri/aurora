//! BYOC / instance OAuth clients (`app_oauth_clients`, 08 §OAuth clients).
//! Secret field values live in the vault behind `secret_ref`.

use super::{After, split};
use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ClientRow {
    pub id: Uuid,
    pub org_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub package_id: String,
    pub client_key: String,
    pub fields: serde_json::Value,
    pub secret_ref: String,
    pub enabled: bool,
    /// The self-hosted instance this client is bound to (D-74).
    pub instance_id: Option<Uuid>,
    pub connection_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const SELECT: &str = "SELECT c.id, c.org_id, c.workspace_id, c.package_id, c.client_key, c.fields, \
    c.secret_ref, c.enabled, c.instance_id, \
    (SELECT count(*) FROM app_connections k WHERE k.oauth_client_id = c.id) AS connection_count, \
    c.created_at, c.updated_at FROM app_oauth_clients c";

pub const SCOPE_CONSTRAINT: &str = "app_oauth_clients_scope_uq";

pub struct NewClient<'a> {
    pub id: Uuid,
    pub org_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub package_id: &'a str,
    pub client_key: &'a str,
    pub fields: &'a serde_json::Value,
    pub secret_ref: &'a str,
    pub instance_id: Option<Uuid>,
}

pub async fn insert<'e, E: PgExecutor<'e>>(exec: E, n: &NewClient<'_>) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO app_oauth_clients (id, org_id, workspace_id, package_id, client_key, fields, secret_ref, instance_id) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(n.id)
    .bind(n.org_id)
    .bind(n.workspace_id)
    .bind(n.package_id)
    .bind(n.client_key)
    .bind(n.fields)
    .bind(n.secret_ref)
    .bind(n.instance_id)
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn get<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<ClientRow>, DbError> {
    Ok(sqlx::query_as(&format!("{SELECT} WHERE c.id = $1"))
        .bind(id)
        .fetch_optional(exec)
        .await?)
}

/// Clients of one scope; `org = None` lists instance clients.
pub async fn list_scope<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Option<Uuid>,
    workspace: Option<Uuid>,
    after: After,
    fetch: i64,
) -> Result<Vec<ClientRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(&format!(
        "{SELECT} WHERE c.org_id IS NOT DISTINCT FROM $1 AND c.workspace_id IS NOT DISTINCT FROM $2 \
         AND ($3::timestamptz IS NULL OR (c.created_at, c.id) < ($3, $4)) \
         ORDER BY c.created_at DESC, c.id DESC LIMIT $5"
    ))
    .bind(org)
    .bind(workspace)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

/// Replace public fields / secret ref / enabled flag (each optional), **compare-and-set**
/// (FIX-115): only while the row is still the one the caller read — same `secret_ref` and
/// `updated_at` (the version of the public fields). Returns false when it changed meanwhile;
/// the caller answers 409 and discards what it sealed.
pub async fn update_cas<'e, E: PgExecutor<'e>>(
    exec: E,
    seen: &ClientRow,
    fields: Option<&serde_json::Value>,
    secret_ref: Option<&str>,
    enabled: Option<bool>,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE app_oauth_clients SET fields = COALESCE($2, fields), \
         secret_ref = COALESCE($3, secret_ref), enabled = COALESCE($4, enabled), \
         updated_at = now() WHERE id = $1 AND secret_ref = $5 AND updated_at = $6",
    )
    .bind(seen.id)
    .bind(fields)
    .bind(secret_ref)
    .bind(enabled)
    .bind(&seen.secret_ref)
    .bind(seen.updated_at)
    .execute(exec)
    .await?
    .rows_affected()
        > 0)
}

pub async fn delete<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query("DELETE FROM app_oauth_clients WHERE id = $1")
        .bind(id)
        .execute(exec)
        .await?
        .rows_affected()
        > 0)
}
