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
    pub requested_scopes: Vec<String>,
    pub metadata: serde_json::Value,
    pub tenant_host: Option<String>,
    pub oauth_client_id: Option<Uuid>,
    /// The registered self-hosted instance (D-74); null for `default_host` and other apps.
    pub instance_id: Option<Uuid>,
    pub instance_host: Option<String>,
    pub instance_label: Option<String>,
    pub credentials_expires_at: Option<DateTime<Utc>>,
    pub connected_by: Option<Uuid>,
    pub connected_by_name: Option<String>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const SELECT: &str = "SELECT c.id, c.org_id, c.workspace_id, c.app_id, c.package_id, c.app_version, \
    c.label, c.status, c.status_reason, c.scopes, c.requested_scopes, c.metadata, c.tenant_host, c.oauth_client_id, \
    c.instance_id, ai.host AS instance_host, ai.label AS instance_label, \
    c.credentials_expires_at, c.connected_by, u.display_name AS connected_by_name, c.last_used_at, \
    c.created_at, c.updated_at FROM app_connections c LEFT JOIN users u ON u.id = c.connected_by \
    LEFT JOIN app_instances ai ON ai.id = c.instance_id";

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

// ---- connect flows (BE-043) ----

pub struct NewConnection<'a> {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub app_id: &'a str,
    pub package_id: &'a str,
    pub app_version: &'a str,
    pub label: &'a str,
    pub credentials_ref: &'a str,
    pub credentials_expires_at: Option<DateTime<Utc>>,
    pub scopes: &'a [String],
    pub requested_scopes: &'a [String],
    pub metadata: &'a serde_json::Value,
    pub tenant_host: Option<&'a str>,
    pub oauth_client_id: Option<Uuid>,
    pub connected_by: Uuid,
    /// The registered app instance (D-74); immutable once set (a reconnect keeps it).
    pub instance_id: Option<Uuid>,
    /// Names (never values) of the fields in the sealed credential JSON (RFE-022).
    pub credential_keys: &'a [String],
}

pub async fn insert<'e, E: PgExecutor<'e>>(exec: E, n: &NewConnection<'_>) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, \
         label, status, credentials_ref, credentials_expires_at, scopes, metadata, tenant_host, \
         oauth_client_id, connected_by, requested_scopes, instance_id, credential_keys) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,'active',$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)",
    )
    .bind(n.id)
    .bind(n.org_id)
    .bind(n.workspace_id)
    .bind(n.app_id)
    .bind(n.package_id)
    .bind(n.app_version)
    .bind(n.label)
    .bind(n.credentials_ref)
    .bind(n.credentials_expires_at)
    .bind(n.scopes)
    .bind(n.metadata)
    .bind(n.tenant_host)
    .bind(n.oauth_client_id)
    .bind(n.connected_by)
    .bind(n.requested_scopes)
    .bind(n.instance_id)
    .bind(n.credential_keys)
    .execute(exec)
    .await?;
    Ok(())
}

/// `(credentials_ref, credentials_version)` of a connection.
pub async fn credentials_state<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<(String, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT credentials_ref, credentials_version FROM app_connections WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(exec)
    .await?)
}

/// Compare-and-set reconnect: replaces credentials and clears `reauth_required`/`orphaned`.
/// False when `credentials_version` moved since `read_version`.
pub async fn reconnect_cas<'e, E: PgExecutor<'e>>(
    exec: E,
    n: &NewConnection<'_>,
    read_version: i64,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE app_connections SET credentials_ref = $3, credentials_version = credentials_version + 1, \
         credentials_expires_at = $4, scopes = $5, metadata = $6, tenant_host = $7, oauth_client_id = $8, \
         package_id = $9, app_version = $10, connected_by = $11, requested_scopes = $12, \
         credential_keys = $14, \
         status_reason = CASE WHEN status IN ('reauth_required','orphaned') THEN NULL ELSE status_reason END, \
         status = CASE WHEN status IN ('reauth_required','orphaned') THEN 'active' ELSE status END, \
         reauth_required_at = NULL, updated_at = now() \
         WHERE id = $1 AND credentials_version = $2 AND instance_id IS NOT DISTINCT FROM $13",
    )
    .bind(n.id)
    .bind(read_version)
    .bind(n.credentials_ref)
    .bind(n.credentials_expires_at)
    .bind(n.scopes)
    .bind(n.metadata)
    .bind(n.tenant_host)
    .bind(n.oauth_client_id)
    .bind(n.package_id)
    .bind(n.app_version)
    .bind(n.connected_by)
    .bind(n.requested_scopes)
    .bind(n.instance_id)
    .bind(n.credential_keys)
    .execute(exec)
    .await?
    .rows_affected()
        == 1)
}

pub async fn org_external_id<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
) -> Result<Option<String>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT external_id FROM organizations WHERE id = $1")
            .bind(org)
            .fetch_optional(exec)
            .await?,
    )
}

/// Organization-level availability of an app (open posture, or allowed org-wide).
pub async fn org_app_available<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    app_id: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT o.app_posture <> 'restricted' OR EXISTS (SELECT 1 FROM org_app_availability a \
         WHERE a.org_id = o.id AND a.workspace_id IS NULL AND a.app_id = $2) \
         FROM organizations o WHERE o.id = $1",
    )
    .bind(org)
    .bind(app_id)
    .fetch_optional(exec)
    .await?
    .unwrap_or(false))
}

/// Enabled OAuth clients usable from a scope, most specific first
/// (workspace, organization, instance). A client bound to a self-hosted instance (D-74) is
/// returned only when `instance` names that instance; with `instance = None` only unbound
/// clients are (the SaaS `default_host` and every non-instance app).
pub async fn usable_clients<'e, E: PgExecutor<'e>>(
    exec: E,
    package_id: &str,
    client_key: &str,
    org: Uuid,
    workspace: Option<Uuid>,
    instance: Option<Uuid>,
) -> Result<Vec<super::oauth_clients::ClientRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT c.id, c.org_id, c.workspace_id, c.package_id, c.client_key, c.fields, c.secret_ref, \
         c.enabled, c.instance_id, 0::bigint AS connection_count, c.created_at, c.updated_at \
         FROM app_oauth_clients c WHERE c.package_id = $1 AND c.client_key = $2 AND c.enabled \
         AND c.instance_id IS NOT DISTINCT FROM $5 \
         AND ((c.org_id IS NULL) OR (c.org_id = $3 AND (c.workspace_id IS NULL OR c.workspace_id = $4))) \
         ORDER BY (c.workspace_id IS NOT NULL) DESC, (c.org_id IS NOT NULL) DESC, c.created_at",
    )
    .bind(package_id)
    .bind(client_key)
    .bind(org)
    .bind(workspace)
    .bind(instance)
    .fetch_all(exec)
    .await?)
}
