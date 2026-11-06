//! App connections (metadata only; credentials stay in the vault, SEC-12).

use super::{After, split};
use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ConnectionRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub app_id: String,
    pub package_id: String,
    pub app_version: String,
    pub label: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub scopes: Vec<String>,
    pub metadata: serde_json::Value,
    pub tenant_host: Option<String>,
    pub oauth_client_id: Option<Uuid>,
    pub credentials_expires_at: Option<DateTime<Utc>>,
    pub connected_by: Option<Uuid>,
    pub connected_by_name: Option<String>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const SELECT: &str = "SELECT c.id, c.org_id, c.workspace_id, c.app_id, c.package_id, c.app_version, \
    c.label, c.status, c.status_reason, c.scopes, c.metadata, c.tenant_host, c.oauth_client_id, \
    c.credentials_expires_at, c.connected_by, u.display_name AS connected_by_name, c.last_used_at, \
    c.created_at, c.updated_at FROM app_connections c LEFT JOIN users u ON u.id = c.connected_by";

pub async fn get<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<ConnectionRow>, DbError> {
    Ok(sqlx::query_as(&format!("{SELECT} WHERE c.id = $1"))
        .bind(id)
        .fetch_optional(exec)
        .await?)
}

/// Connections of one scope (`workspace = None` = organization scope).
#[allow(clippy::too_many_arguments)]
pub async fn list_scope<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    workspace: Option<Uuid>,
    app_id: Option<&str>,
    status: Option<&str>,
    after: After,
    fetch: i64,
) -> Result<Vec<ConnectionRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(&format!(
        "{SELECT} WHERE c.org_id = $1 AND c.workspace_id IS NOT DISTINCT FROM $2 \
         AND ($3::text IS NULL OR c.app_id = $3) AND ($4::text IS NULL OR c.status = $4) \
         AND ($5::timestamptz IS NULL OR (c.created_at, c.id) < ($5, $6)) \
         ORDER BY c.created_at DESC, c.id DESC LIMIT $7"
    ))
    .bind(org)
    .bind(workspace)
    .bind(app_id)
    .bind(status)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

/// Connection counts per app visible from a workspace (its own + organization-level).
pub async fn counts_for_workspace<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    workspace: Uuid,
) -> Result<Vec<(String, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT app_id, count(*) FROM app_connections WHERE org_id = $1 \
         AND (workspace_id IS NULL OR workspace_id = $2) AND status <> 'orphaned' GROUP BY app_id",
    )
    .bind(org)
    .bind(workspace)
    .fetch_all(exec)
    .await?)
}

/// Update label and/or status; returns the new row.
pub async fn update<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    label: Option<&str>,
    status: Option<&str>,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE app_connections SET label = COALESCE($2, label), \
         status = COALESCE($3, status), \
         status_reason = CASE WHEN $3 IS NULL THEN status_reason \
                              WHEN $3 = 'disabled' THEN 'disabled_by_user' ELSE NULL END, \
         reauth_required_at = CASE WHEN $3 = 'active' THEN NULL ELSE reauth_required_at END, \
         updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(label)
    .bind(status)
    .execute(exec)
    .await?
    .rows_affected()
        > 0)
}

/// The connection and the connections derived from it (`source_connection_id`).
pub async fn with_children(conn: &mut PgConnection, id: Uuid) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM app_connections WHERE id = $1 OR source_connection_id = $1",
    )
    .bind(id)
    .fetch_all(conn)
    .await?)
}

/// Agents holding a grant on any of the connections.
pub async fn granted_agents(conn: &mut PgConnection, ids: &[Uuid]) -> Result<Vec<Uuid>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT DISTINCT agent_id FROM grants WHERE connection_id = ANY($1)")
            .bind(ids)
            .fetch_all(conn)
            .await?,
    )
}

/// Remove derived rules owned by the connections (blocklist) or by grants on them.
pub async fn delete_derived_rules(conn: &mut PgConnection, ids: &[Uuid]) -> Result<(), DbError> {
    sqlx::query(
        "DELETE FROM derived_rules WHERE (source_kind = 'blocklist' AND source_id = ANY($1)) \
         OR (source_kind = 'grant' AND source_id IN \
             (SELECT id FROM grants WHERE connection_id = ANY($1)))",
    )
    .bind(ids)
    .execute(conn)
    .await?;
    Ok(())
}

/// Delete the connections (grants cascade).
pub async fn delete_many(conn: &mut PgConnection, ids: &[Uuid]) -> Result<(), DbError> {
    sqlx::query("DELETE FROM app_connections WHERE id = ANY($1)")
        .bind(ids)
        .execute(conn)
        .await?;
    Ok(())
}

/// `(id, org_id, workspace_id, credentials_ref)` of the connections.
pub async fn credential_refs(
    conn: &mut PgConnection,
    ids: &[Uuid],
) -> Result<Vec<(Uuid, Uuid, Option<Uuid>, String)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, org_id, workspace_id, credentials_ref FROM app_connections WHERE id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}
