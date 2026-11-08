use super::notify_auth_revoked;
use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct KeyRecord {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub user_id: Uuid,
    /// `user` | `service`.
    pub kind: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    /// Not revoked/expired, owner not disabled and an *active* member of the key's org.
    pub live: bool,
    pub user_email: String,
    pub is_instance_admin: bool,
}

pub async fn find_by_hash<'e, E: PgExecutor<'e>>(
    exec: E,
    hash: &[u8; 32],
) -> Result<Option<KeyRecord>, DbError> {
    Ok(sqlx::query_as(
        "SELECT k.id, k.org_id, k.workspace_id, k.user_id, k.kind, k.expires_at, k.revoked_at,
                (k.revoked_at IS NULL AND (k.expires_at IS NULL OR k.expires_at > now())
                 AND u.disabled_at IS NULL
                 AND EXISTS (SELECT 1 FROM org_members m
                             WHERE m.org_id = k.org_id AND m.user_id = k.user_id
                               AND m.status = 'active')) AS live,
                u.email::text AS user_email, u.is_instance_admin
         FROM api_keys k JOIN users u ON u.id = k.user_id
         WHERE k.token_hash = $1",
    )
    .bind(&hash[..])
    .fetch_optional(exec)
    .await?)
}

/// Update `last_used_at` at most once per minute.
pub async fn touch<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<bool, DbError> {
    let r = sqlx::query(
        "UPDATE api_keys SET last_used_at = now()
         WHERE id = $1 AND (last_used_at IS NULL OR last_used_at < now() - interval '1 minute')",
    )
    .bind(id)
    .execute(exec)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// Revoke a key and notify. Returns whether it was newly revoked.
pub async fn revoke(conn: &mut PgConnection, id: Uuid) -> Result<bool, DbError> {
    let r = sqlx::query(
        "UPDATE api_keys SET revoked_at = now(), updated_at = now()
         WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    notify_auth_revoked(&mut *conn, "api_key", id).await?;
    Ok(r.rows_affected() > 0)
}

// ---- management (BE-014) -----------------------------------------------------

/// A key as listed/managed (never includes the token or its hash).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ApiKeyRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub user_id: Uuid,
    pub kind: String,
    pub name: String,
    pub display_prefix: String,
    pub owner_display_name: String,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

const SEL: &str = "k.id, k.org_id, k.workspace_id, k.user_id, k.kind, k.name, k.display_prefix,
    u.display_name AS owner_display_name, k.last_used_at, k.expires_at, k.revoked_at, k.created_at";

pub struct NewKey<'a> {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub user_id: Uuid,
    pub name: &'a str,
    pub token_hash: &'a [u8; 32],
    pub display_prefix: &'a str,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Insert a `user`-kind key and return it.
pub async fn insert<'e, E: PgExecutor<'e>>(exec: E, k: &NewKey<'_>) -> Result<ApiKeyRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "WITH k AS (
             INSERT INTO api_keys (id, org_id, workspace_id, user_id, kind, name, token_hash,
                                   display_prefix, expires_at)
             VALUES ($1,$2,$3,$4,'user',$5,$6,$7,$8)
             RETURNING *)
         SELECT {SEL} FROM k JOIN users u ON u.id = k.user_id"
    ))
    .bind(k.id)
    .bind(k.org_id)
    .bind(k.workspace_id)
    .bind(k.user_id)
    .bind(k.name)
    .bind(&k.token_hash[..])
    .bind(k.display_prefix)
    .bind(k.expires_at)
    .fetch_one(exec)
    .await?)
}

pub async fn get<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<ApiKeyRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SEL} FROM api_keys k JOIN users u ON u.id = k.user_id WHERE k.id = $1"
    ))
    .bind(id)
    .fetch_optional(exec)
    .await?)
}

/// Org keys (`workspace_id IS NULL`, kind `user`) of `org`.
pub async fn list_org<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    after: super::After,
    fetch: i64,
) -> Result<Vec<ApiKeyRow>, DbError> {
    let (t, id) = super::split(after);
    Ok(sqlx::query_as(&format!(
        "SELECT {SEL} FROM api_keys k JOIN users u ON u.id = k.user_id
         WHERE k.org_id = $1 AND k.workspace_id IS NULL AND k.kind = 'user'
           AND ($2::timestamptz IS NULL OR (k.created_at, k.id) < ($2, $3))
         ORDER BY k.created_at DESC, k.id DESC LIMIT $4"
    ))
    .bind(org)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

/// Workspace keys (kind `user`) of `ws`.
pub async fn list_workspace<'e, E: PgExecutor<'e>>(
    exec: E,
    ws: Uuid,
    after: super::After,
    fetch: i64,
) -> Result<Vec<ApiKeyRow>, DbError> {
    let (t, id) = super::split(after);
    Ok(sqlx::query_as(&format!(
        "SELECT {SEL} FROM api_keys k JOIN users u ON u.id = k.user_id
         WHERE k.workspace_id = $1 AND k.kind = 'user'
           AND ($2::timestamptz IS NULL OR (k.created_at, k.id) < ($2, $3))
         ORDER BY k.created_at DESC, k.id DESC LIMIT $4"
    ))
    .bind(ws)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

/// The user's own `user`-kind keys across orgs/workspaces.
pub async fn list_for_user<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    after: super::After,
    fetch: i64,
) -> Result<Vec<ApiKeyRow>, DbError> {
    let (t, id) = super::split(after);
    Ok(sqlx::query_as(&format!(
        "SELECT {SEL} FROM api_keys k JOIN users u ON u.id = k.user_id
         WHERE k.user_id = $1 AND k.kind = 'user'
           AND ($2::timestamptz IS NULL OR (k.created_at, k.id) < ($2, $3))
         ORDER BY k.created_at DESC, k.id DESC LIMIT $4"
    ))
    .bind(user)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

/// Whether API key `id` is still live (same rule as [`KeyRecord::live`]); unknown => false.
pub async fn is_live<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<bool, DbError> {
    let live: Option<bool> = sqlx::query_scalar(
        "SELECT (k.revoked_at IS NULL AND (k.expires_at IS NULL OR k.expires_at > now())
                 AND u.disabled_at IS NULL
                 AND EXISTS (SELECT 1 FROM org_members m
                             WHERE m.org_id = k.org_id AND m.user_id = k.user_id
                               AND m.status = 'active'))
         FROM api_keys k JOIN users u ON u.id = k.user_id WHERE k.id = $1",
    )
    .bind(id)
    .fetch_optional(exec)
    .await?;
    Ok(live.unwrap_or(false))
}
