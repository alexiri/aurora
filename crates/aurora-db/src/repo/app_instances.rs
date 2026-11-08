//! Self-hosted app instances (`app_instances`, docs/03, D-74): hosts registered for one app by
//! an instance admin (instance-wide, `org_id` null) or an organization's owners/admins.

use super::{After, split};
use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct InstanceRow {
    pub id: Uuid,
    pub org_id: Option<Uuid>,
    pub package_id: String,
    pub app_id: String,
    pub host: String,
    pub label: String,
    pub status: String,
    pub created_by: Option<Uuid>,
    pub created_by_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub connections: i64,
}

const SELECT: &str = "SELECT i.id, i.org_id, i.package_id, i.app_id, i.host, i.label, i.status, \
    i.created_by, u.display_name AS created_by_name, i.created_at, i.updated_at, \
    (SELECT count(*) FROM app_connections c WHERE c.instance_id = i.id) AS connections \
    FROM app_instances i LEFT JOIN users u ON u.id = i.created_by";

pub struct NewInstance<'a> {
    pub id: Uuid,
    pub org_id: Option<Uuid>,
    pub package_id: &'a str,
    pub app_id: &'a str,
    pub host: &'a str,
    pub label: &'a str,
    pub created_by: Option<Uuid>,
}

/// Serialize registrations of one host across scopes (the cross-scope uniqueness is enforced by
/// the API inside the creating transaction; the unique index only covers one scope).
pub async fn lock_host(conn: &mut PgConnection, host: &str) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('app_instance:' || $1, 0))")
        .bind(host)
        .execute(conn)
        .await?;
    Ok(())
}

/// `(app_id, org_id)` of an instance with this host in a scope that can see `org` (instance-wide
/// rows, and the org's own); for an instance-wide registration (`org = None`) any scope.
pub async fn visible_with_host<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Option<Uuid>,
    host: &str,
) -> Result<Option<(String, Option<Uuid>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT app_id, org_id FROM app_instances WHERE host = $2 \
         AND ($1::uuid IS NULL OR org_id IS NULL OR org_id = $1) LIMIT 1",
    )
    .bind(org)
    .bind(host)
    .fetch_optional(exec)
    .await?)
}

pub async fn insert<'e, E: PgExecutor<'e>>(exec: E, n: &NewInstance<'_>) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO app_instances (id, org_id, package_id, app_id, host, label, created_by) \
         VALUES ($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(n.id)
    .bind(n.org_id)
    .bind(n.package_id)
    .bind(n.app_id)
    .bind(n.host)
    .bind(n.label)
    .bind(n.created_by)
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn get<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<InstanceRow>, DbError> {
    Ok(sqlx::query_as(&format!("{SELECT} WHERE i.id = $1"))
        .bind(id)
        .fetch_optional(exec)
        .await?)
}

/// Instances usable in `org` (instance-wide and the org's own), optionally of one app.
pub async fn list_for_org<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    app_id: Option<&str>,
    after: After,
    fetch: i64,
) -> Result<Vec<InstanceRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(&format!(
        "{SELECT} WHERE (i.org_id IS NULL OR i.org_id = $1) AND ($2::text IS NULL OR i.app_id = $2) \
         AND ($3::timestamptz IS NULL OR (i.created_at, i.id) < ($3, $4)) \
         ORDER BY i.created_at DESC, i.id DESC LIMIT $5"
    ))
    .bind(org)
    .bind(app_id)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

/// Instance-wide instances (`org_id` null).
pub async fn list_instance_wide<'e, E: PgExecutor<'e>>(
    exec: E,
    app_id: Option<&str>,
    after: After,
    fetch: i64,
) -> Result<Vec<InstanceRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(&format!(
        "{SELECT} WHERE i.org_id IS NULL AND ($1::text IS NULL OR i.app_id = $1) \
         AND ($2::timestamptz IS NULL OR (i.created_at, i.id) < ($2, $3)) \
         ORDER BY i.created_at DESC, i.id DESC LIMIT $4"
    ))
    .bind(app_id)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

/// Update label and/or status.
pub async fn update<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    label: Option<&str>,
    status: Option<&str>,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE app_instances SET label = COALESCE($2, label), status = COALESCE($3, status), \
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

/// First half of a deletion (C-242), inside the caller's transaction: the instance's connections
/// become `disabled` with `status_reason = instance_deleted` and lose their instance and client
/// binding (so the rows can go: restrict FKs). Returns the affected connection ids.
pub async fn disable_connections(conn: &mut PgConnection, id: Uuid) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE app_connections SET status = 'disabled', status_reason = 'instance_deleted', \
         instance_id = NULL, oauth_client_id = NULL, updated_at = now() \
         WHERE instance_id = $1 \
            OR oauth_client_id IN (SELECT id FROM app_oauth_clients WHERE instance_id = $1) \
         RETURNING id",
    )
    .bind(id)
    .fetch_all(conn)
    .await?)
}

/// Delete the instance; its BYOC clients go with it (cascade).
pub async fn delete(conn: &mut PgConnection, id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query("DELETE FROM app_instances WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?
        .rows_affected()
        > 0)
}

/// Ids of the OAuth clients bound to the instance (sealed secrets to discard after deletion).
pub async fn client_secret_refs(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Vec<(Uuid, Option<Uuid>, Option<Uuid>, String)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, org_id, workspace_id, secret_ref FROM app_oauth_clients WHERE instance_id = $1",
    )
    .bind(id)
    .fetch_all(conn)
    .await?)
}
