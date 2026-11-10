//! Workspaces and access bindings.

use super::{After, split};
use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WorkspaceRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub slug: String,
    pub name: String,
    pub timezone: String,
    pub memory_language: String,
    /// (RFE-025) `enabled` | `owners_only` | `disabled`; null = inherit.
    pub byo_posture: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str =
    "id, org_id, slug, name, timezone, memory_language, byo_posture, created_at, updated_at";

pub const SLUG_CONSTRAINT: &str = "workspaces_org_id_slug_key";

pub async fn insert<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    org: Uuid,
    slug: &str,
    name: &str,
    timezone: &str,
    memory_language: &str,
) -> Result<WorkspaceRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO workspaces (id, org_id, slug, name, timezone, memory_language)
         VALUES ($1,$2,$3,$4,$5,$6) RETURNING {COLS}"
    ))
    .bind(id)
    .bind(org)
    .bind(slug)
    .bind(name)
    .bind(timezone)
    .bind(memory_language)
    .fetch_one(exec)
    .await?)
}

pub async fn slug_exists<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    slug: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM workspaces WHERE org_id = $1 AND slug = $2)",
    )
    .bind(org)
    .bind(slug)
    .fetch_one(exec)
    .await?)
}

pub async fn get<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<WorkspaceRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("SELECT {COLS} FROM workspaces WHERE id = $1"))
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}

pub async fn get_by_slug<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    slug: &str,
) -> Result<Option<WorkspaceRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM workspaces WHERE org_id = $1 AND slug = $2"
    ))
    .bind(org)
    .bind(slug)
    .fetch_optional(exec)
    .await?)
}

/// Workspaces of `org` visible to `user`: all for org owner/admin (`all`),
/// otherwise those with a binding.
pub async fn list_visible<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    user: Uuid,
    all: bool,
    after: After,
    fetch: i64,
) -> Result<Vec<WorkspaceRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM workspaces w WHERE w.org_id = $1
         AND ($2 OR EXISTS (SELECT 1 FROM workspace_access a WHERE a.workspace_id = w.id AND a.user_id = $3))
         AND ($4::timestamptz IS NULL OR (w.created_at, w.id) < ($4, $5))
         ORDER BY w.created_at DESC, w.id DESC LIMIT $6"
    ))
    .bind(org)
    .bind(all)
    .bind(user)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

pub async fn update<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    name: Option<&str>,
    timezone: Option<&str>,
    memory_language: Option<&str>,
) -> Result<WorkspaceRow, DbError> {
    Ok(sqlx::query_as(&format!(
        "UPDATE workspaces SET name = COALESCE($2, name), timezone = COALESCE($3, timezone),
         home_sync_generation = home_sync_generation
             + CASE WHEN $4::text IS NOT NULL AND $4::text <> memory_language THEN 1 ELSE 0 END,
         memory_language = COALESCE($4, memory_language), updated_at = now()
         WHERE id = $1 RETURNING {COLS}"
    ))
    .bind(id)
    .bind(name)
    .bind(timezone)
    .bind(memory_language)
    .fetch_one(exec)
    .await?)
}

pub async fn delete<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<(), DbError> {
    sqlx::query("DELETE FROM workspaces WHERE id = $1")
        .bind(id)
        .execute(exec)
        .await?;
    Ok(())
}

pub async fn memory_language_exists<'e, E: PgExecutor<'e>>(
    exec: E,
    name: &str,
) -> Result<bool, DbError> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_ts_config WHERE cfgname = $1)")
            .bind(name)
            .fetch_one(exec)
            .await?,
    )
}

pub async fn bind_user<'e, E: PgExecutor<'e>>(
    exec: E,
    ws: Uuid,
    user: Uuid,
    role: &str,
) -> Result<(), DbError> {
    sqlx::query(
        // P4-IDP-1: on a claim-managed row a manual role goes to `base_role` and the effective
        // role stays the stronger of the two, so a binding never undercuts the claim.
        "INSERT INTO workspace_access (workspace_id, user_id, role) VALUES ($1,$2,$3)
         ON CONFLICT (workspace_id, user_id) DO UPDATE SET
           role = CASE WHEN workspace_access.idp_role IS NULL THEN EXCLUDED.role
             WHEN array_position(ARRAY['member','approver','owner'], EXCLUDED.role)
                >= array_position(ARRAY['member','approver','owner'], workspace_access.idp_role)
             THEN EXCLUDED.role ELSE workspace_access.idp_role END,
           base_role = CASE WHEN workspace_access.idp_role IS NULL THEN workspace_access.base_role
             ELSE EXCLUDED.role END,
           updated_at = now()",
    )
    .bind(ws)
    .bind(user)
    .bind(role)
    .execute(exec)
    .await?;
    Ok(())
}

/// Explicit workspace roles of `user` among `ids` (phase 2, RFE-012: member/approver/owner).
pub async fn roles_of<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    ids: &[Uuid],
) -> Result<Vec<(Uuid, String)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT workspace_id, role FROM workspace_access WHERE user_id = $1 AND workspace_id = ANY($2)",
    )
    .bind(user)
    .bind(ids)
    .fetch_all(exec)
    .await?)
}

/// Returns whether a row was removed.
pub async fn unbind_user<'e, E: PgExecutor<'e>>(
    exec: E,
    ws: Uuid,
    user: Uuid,
) -> Result<bool, DbError> {
    Ok(
        sqlx::query("DELETE FROM workspace_access WHERE workspace_id = $1 AND user_id = $2")
            .bind(ws)
            .bind(user)
            .execute(exec)
            .await?
            .rows_affected()
            > 0,
    )
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AccessRow {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub implicit: bool,
    pub created_at: DateTime<Utc>,
    /// (P4-IDP-1) The IdP role value behind this binding; `None` for manual bindings.
    pub idp_value: Option<String>,
}

const ACCESS_SEL: &str = "SELECT u.id AS user_id, u.email::text AS email, u.display_name,
    CASE WHEN m.role IN ('owner','admin') THEN 'owner' ELSE a.role END AS role,
    (m.role IN ('owner','admin')) AS implicit,
    COALESCE(a.created_at, m.created_at) AS created_at,
    substring(a.idp_source from '^idp:[^:]*:(.*)$') AS idp_value
  FROM workspaces w
  JOIN org_members m ON m.org_id = w.org_id AND m.status = 'active'
  JOIN users u ON u.id = m.user_id
  LEFT JOIN workspace_access a ON a.workspace_id = w.id AND a.user_id = m.user_id
  WHERE w.id = $1 AND (m.role IN ('owner','admin') OR a.user_id IS NOT NULL)";

/// Effective bindings (explicit rows of active members + implicit org owners/admins).
pub async fn list_access<'e, E: PgExecutor<'e>>(
    exec: E,
    ws: Uuid,
    after: After,
    fetch: i64,
) -> Result<Vec<AccessRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(&format!(
        "SELECT * FROM ({ACCESS_SEL}) x
         WHERE ($2::timestamptz IS NULL OR (x.created_at, x.user_id) < ($2, $3))
         ORDER BY x.created_at DESC, x.user_id DESC LIMIT $4"
    ))
    .bind(ws)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

pub async fn get_access<'e, E: PgExecutor<'e>>(
    exec: E,
    ws: Uuid,
    user: Uuid,
) -> Result<Option<AccessRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT * FROM ({ACCESS_SEL}) x WHERE x.user_id = $2"
    ))
    .bind(ws)
    .bind(user)
    .fetch_optional(exec)
    .await?)
}

/// (RFE-025) Sets the workspace BYO posture (`None` = inherit).
pub async fn set_byo_posture<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    posture: Option<&str>,
) -> Result<Option<WorkspaceRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "UPDATE workspaces SET byo_posture = $2, updated_at = now() WHERE id = $1 RETURNING {COLS}"
    ))
    .bind(id)
    .bind(posture)
    .fetch_optional(exec)
    .await?)
}
