//! Roles derived from IdP claims (P4-IDP-1, D-139, SEC-78).
//!
//! Claim-sourced assignments are tracked on the rows they produce (`idp_role`, `idp_source`,
//! `base_role` on `org_members` and `workspace_access`, `users.idp_admin_source`). `role` stays
//! the effective role every other reader uses. The sync only ever touches rows whose
//! `idp_source` carries the signing-in provider's prefix.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

/// `idp:<provider>:` — the prefix of every `idp_source` written for a provider.
pub fn source_prefix(provider: &str) -> String {
    format!("idp:{provider}:")
}

pub fn source(provider: &str, value: &str) -> String {
    format!("idp:{provider}:{value}")
}

/// Strength order of org roles (higher is stronger).
pub fn org_rank(role: &str) -> u8 {
    match role {
        "owner" => 3,
        "admin" => 2,
        _ => 1,
    }
}

/// Strength order of workspace roles.
pub fn ws_rank(role: &str) -> u8 {
    match role {
        "owner" => 3,
        "approver" => 2,
        _ => 1,
    }
}

/// An org membership row with its claim-tracking columns.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RoleRow {
    pub role: String,
    pub status: String,
    pub idp_role: Option<String>,
    pub idp_source: Option<String>,
    pub base_role: Option<String>,
}

pub async fn org_row<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    user: Uuid,
) -> Result<Option<RoleRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT role, status, idp_role, idp_source, base_role FROM org_members
         WHERE org_id = $1 AND user_id = $2 FOR UPDATE",
    )
    .bind(org)
    .bind(user)
    .fetch_optional(exec)
    .await?)
}

/// Insert a membership the claim created (no other source: `base_role` stays NULL).
pub async fn org_insert<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    user: Uuid,
    role: &str,
    source: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO org_members (org_id, user_id, role, idp_role, idp_source)
         VALUES ($1, $2, $3, $3, $4)",
    )
    .bind(org)
    .bind(user)
    .bind(role)
    .bind(source)
    .execute(exec)
    .await?;
    Ok(())
}

/// Write the effective role and the claim-tracking columns of an existing membership.
pub async fn org_write<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    user: Uuid,
    row: &RoleRow,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE org_members SET role = $3, idp_role = $4, idp_source = $5, base_role = $6,
         updated_at = now() WHERE org_id = $1 AND user_id = $2",
    )
    .bind(org)
    .bind(user)
    .bind(&row.role)
    .bind(&row.idp_role)
    .bind(&row.idp_source)
    .bind(&row.base_role)
    .execute(exec)
    .await?;
    Ok(())
}

/// Whether the membership is claim-managed (a manual edit or removal is refused).
pub async fn org_is_managed<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    user: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE((SELECT idp_role IS NOT NULL FROM org_members
         WHERE org_id = $1 AND user_id = $2), false)",
    )
    .bind(org)
    .bind(user)
    .fetch_one(exec)
    .await?)
}

pub async fn ws_is_managed<'e, E: PgExecutor<'e>>(
    exec: E,
    ws: Uuid,
    user: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE((SELECT idp_role IS NOT NULL FROM workspace_access
         WHERE workspace_id = $1 AND user_id = $2), false)",
    )
    .bind(ws)
    .bind(user)
    .fetch_one(exec)
    .await?)
}

/// A claim-sourced org membership of `user` for the provider.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OrgClaimRow {
    pub org_id: Uuid,
    pub slug: String,
    pub idp_source: String,
}

pub async fn org_claim_rows<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    provider: &str,
) -> Result<Vec<OrgClaimRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT m.org_id, o.slug, m.idp_source FROM org_members m
         JOIN organizations o ON o.id = m.org_id
         WHERE m.user_id = $1 AND starts_with(m.idp_source, $2) ORDER BY o.slug",
    )
    .bind(user)
    .bind(source_prefix(provider))
    .fetch_all(exec)
    .await?)
}

pub async fn ws_row<'e, E: PgExecutor<'e>>(
    exec: E,
    ws: Uuid,
    user: Uuid,
) -> Result<Option<RoleRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT role, 'active'::text AS status, idp_role, idp_source, base_role
         FROM workspace_access WHERE workspace_id = $1 AND user_id = $2 FOR UPDATE",
    )
    .bind(ws)
    .bind(user)
    .fetch_optional(exec)
    .await?)
}

pub async fn ws_insert<'e, E: PgExecutor<'e>>(
    exec: E,
    ws: Uuid,
    user: Uuid,
    role: &str,
    source: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO workspace_access (workspace_id, user_id, role, idp_role, idp_source)
         VALUES ($1, $2, $3, $3, $4)",
    )
    .bind(ws)
    .bind(user)
    .bind(role)
    .bind(source)
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn ws_write<'e, E: PgExecutor<'e>>(
    exec: E,
    ws: Uuid,
    user: Uuid,
    row: &RoleRow,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE workspace_access SET role = $3, idp_role = $4, idp_source = $5, base_role = $6,
         updated_at = now() WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(ws)
    .bind(user)
    .bind(&row.role)
    .bind(&row.idp_role)
    .bind(&row.idp_source)
    .bind(&row.base_role)
    .execute(exec)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WsClaimRow {
    pub workspace_id: Uuid,
    pub org_id: Uuid,
    pub org_slug: String,
    pub slug: String,
    pub idp_source: String,
}

pub async fn ws_claim_rows<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    provider: &str,
) -> Result<Vec<WsClaimRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT a.workspace_id, w.org_id, o.slug AS org_slug, w.slug, a.idp_source
         FROM workspace_access a JOIN workspaces w ON w.id = a.workspace_id
         JOIN organizations o ON o.id = w.org_id
         WHERE a.user_id = $1 AND starts_with(a.idp_source, $2) ORDER BY o.slug, w.slug",
    )
    .bind(user)
    .bind(source_prefix(provider))
    .fetch_all(exec)
    .await?)
}

pub async fn ws_lookup<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    slug: &str,
) -> Result<Option<Uuid>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT id FROM workspaces WHERE org_id = $1 AND slug = $2")
            .bind(org)
            .bind(slug)
            .fetch_optional(exec)
            .await?,
    )
}

/// The instance-admin flag's claim source of `user` (`None`: not claim-granted).
pub async fn admin_source<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
) -> Result<Option<String>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT idp_admin_source FROM users WHERE id = $1")
            .bind(user)
            .fetch_one(exec)
            .await?,
    )
}

/// Set the flag and its claim source together (`source = None` with `admin = false` revokes).
pub async fn set_admin<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    admin: bool,
    source: Option<&str>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE users SET is_instance_admin = $2, idp_admin_source = $3, updated_at = now()
         WHERE id = $1",
    )
    .bind(user)
    .bind(admin)
    .bind(source)
    .execute(exec)
    .await?;
    Ok(())
}

/// A manual grant took the flag over: it no longer depends on the claim.
pub async fn clear_admin_source<'e, E: PgExecutor<'e>>(exec: E, user: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE users SET idp_admin_source = NULL WHERE id = $1 AND idp_admin_source IS NOT NULL",
    )
    .bind(user)
    .execute(exec)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SyncState {
    pub user_id: Uuid,
    pub provider_id: String,
    pub claim_values: Vec<String>,
    pub granted: serde_json::Value,
    pub synced_at: DateTime<Utc>,
}

pub async fn save_state(
    conn: &mut PgConnection,
    user: Uuid,
    provider: &str,
    values: &[String],
    granted: &serde_json::Value,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO user_idp_roles (user_id, provider_id, claim_values, granted, synced_at)
         VALUES ($1, $2, $3, $4, now())
         ON CONFLICT (user_id) DO UPDATE SET provider_id = EXCLUDED.provider_id,
           claim_values = EXCLUDED.claim_values, granted = EXCLUDED.granted, synced_at = now()",
    )
    .bind(user)
    .bind(provider)
    .bind(values)
    .bind(granted)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub async fn get_state<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
) -> Result<Option<SyncState>, DbError> {
    Ok(sqlx::query_as(
        "SELECT user_id, provider_id, claim_values, granted, synced_at FROM user_idp_roles
         WHERE user_id = $1",
    )
    .bind(user)
    .fetch_optional(exec)
    .await?)
}

/// States of many users (the admin Users page).
pub async fn states_of<'e, E: PgExecutor<'e>>(
    exec: E,
    users: &[Uuid],
) -> Result<Vec<SyncState>, DbError> {
    Ok(sqlx::query_as(
        "SELECT user_id, provider_id, claim_values, granted, synced_at FROM user_idp_roles
         WHERE user_id = ANY($1)",
    )
    .bind(users)
    .fetch_all(exec)
    .await?)
}
