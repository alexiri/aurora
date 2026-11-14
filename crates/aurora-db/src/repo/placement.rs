//! New-user placement (P4-ONB-1, D-140, SEC-79): the saved instance placement and personal
//! workspaces.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PlacementRow {
    pub mode: String,
    pub default_org_id: Option<Uuid>,
    pub default_org_role: String,
    pub shared_workspaces: serde_json::Value,
    pub personal_enabled: bool,
    pub personal_role: String,
    pub name_template: String,
    pub starter_grants: serde_json::Value,
    pub updated_by: Option<Uuid>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str = "mode, default_org_id, default_org_role, shared_workspaces, personal_enabled, \
    personal_role, name_template, starter_grants, updated_by, updated_at";

pub async fn get<'e, E: PgExecutor<'e>>(exec: E) -> Result<Option<PlacementRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("SELECT {COLS} FROM instance_placement"))
            .fetch_optional(exec)
            .await?,
    )
}

/// Insert or replace the single placement row.
pub async fn save<'e, E: PgExecutor<'e>>(
    exec: E,
    p: &PlacementRow,
    by: Uuid,
) -> Result<(), DbError> {
    sqlx::query(
        "WITH setup AS (UPDATE instance_setup SET placement_pending = false, updated_at = now()
                         WHERE placement_pending)
         INSERT INTO instance_placement (singleton, mode, default_org_id, default_org_role,
            shared_workspaces, personal_enabled, personal_role, name_template, starter_grants,
            updated_by, updated_at)
         VALUES (true, $1,$2,$3,$4,$5,$6,$7,$8,$9, now())
         ON CONFLICT (singleton) DO UPDATE SET mode = $1, default_org_id = $2,
            default_org_role = $3, shared_workspaces = $4, personal_enabled = $5,
            personal_role = $6, name_template = $7, starter_grants = $8, updated_by = $9,
            updated_at = now()",
    )
    .bind(&p.mode)
    .bind(p.default_org_id)
    .bind(&p.default_org_role)
    .bind(&p.shared_workspaces)
    .bind(p.personal_enabled)
    .bind(&p.personal_role)
    .bind(&p.name_template)
    .bind(&p.starter_grants)
    .bind(by)
    .execute(exec)
    .await?;
    Ok(())
}

/// Serialize placement work per organization (concurrent first sign-ins).
pub async fn lock_org<'e, E: PgExecutor<'e>>(exec: E, org: Uuid) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('placement:' || $1::text, 0))")
        .bind(org.to_string())
        .execute(exec)
        .await?;
    Ok(())
}

/// The user's personal workspace in `org`, found **only** by the recorded owner id (SEC-79).
pub async fn personal_workspace<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    user: Uuid,
) -> Result<Option<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM workspaces WHERE org_id = $1 AND personal_owner_user_id = $2",
    )
    .bind(org)
    .bind(user)
    .fetch_optional(exec)
    .await?)
}

/// (FIX-097) Is the workspace a personal one? The sticky `personal` marker, not the owner id:
/// that is nulled when the person's account is deleted, and the workspace must still never be
/// handed out as a shared workspace.
pub async fn is_personal<'e, E: PgExecutor<'e>>(exec: E, workspace: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT personal OR personal_owner_user_id IS NOT NULL FROM workspaces WHERE id = $1",
    )
    .bind(workspace)
    .fetch_optional(exec)
    .await?
    .unwrap_or(false))
}

/// The user a workspace is the personal workspace of, if any.
pub async fn personal_owner_of<'e, E: PgExecutor<'e>>(
    exec: E,
    workspace: Uuid,
) -> Result<Option<Uuid>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT personal_owner_user_id FROM workspaces WHERE id = $1")
            .bind(workspace)
            .fetch_optional(exec)
            .await?
            .flatten(),
    )
}

pub async fn set_personal_owner<'e, E: PgExecutor<'e>>(
    exec: E,
    workspace: Uuid,
    user: Uuid,
) -> Result<(), DbError> {
    sqlx::query("UPDATE workspaces SET personal_owner_user_id = $2, personal = true WHERE id = $1")
        .bind(workspace)
        .bind(user)
        .execute(exec)
        .await?;
    Ok(())
}

/// `(slug, name)` of an organization.
pub async fn org_brief<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
) -> Result<Option<(String, String)>, DbError> {
    Ok(
        sqlx::query_as("SELECT slug, name FROM organizations WHERE id = $1")
            .bind(org)
            .fetch_optional(exec)
            .await?,
    )
}

/// What the admin may pick in `org`: ordinary workspaces (never personal ones) and org-level
/// credentials. Capped; names only.
pub async fn choices<'e, E: PgExecutor<'e> + Copy>(exec: E, org: Uuid) -> Result<Choices, DbError> {
    let workspaces = sqlx::query_as(
        "SELECT id, slug, name FROM workspaces WHERE org_id = $1 AND NOT personal
           AND personal_owner_user_id IS NULL
         ORDER BY name, id LIMIT 500",
    )
    .bind(org)
    .fetch_all(exec)
    .await?;
    let secrets = sqlx::query_as(
        "SELECT id, name, kind FROM secrets WHERE org_id = $1 AND workspace_id IS NULL
         AND status = 'active' ORDER BY name, id LIMIT 500",
    )
    .bind(org)
    .fetch_all(exec)
    .await?;
    let connections = sqlx::query_as(
        "SELECT id, label, app_id FROM app_connections WHERE org_id = $1 AND workspace_id IS NULL
         AND status <> 'orphaned' ORDER BY label, id LIMIT 500",
    )
    .bind(org)
    .fetch_all(exec)
    .await?;
    Ok(Choices {
        workspaces,
        secrets,
        connections,
    })
}

#[derive(Debug, Clone)]
pub struct Choices {
    pub workspaces: Vec<(Uuid, String, String)>,
    pub secrets: Vec<(Uuid, String, String)>,
    pub connections: Vec<(Uuid, String, String)>,
}

/// Whether a placement has still to be saved on an instance installed with this release
/// (FIX-116): ordinary users are refused with `instance_setup_pending` until it is. A missing
/// row (never written) counts as not pending.
pub async fn setup_pending<'e, E: PgExecutor<'e>>(exec: E) -> Result<bool, DbError> {
    Ok(
        sqlx::query_scalar("SELECT placement_pending FROM instance_setup")
            .fetch_optional(exec)
            .await?
            .unwrap_or(false),
    )
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PersonalOrg {
    pub org_id: Uuid,
    pub slug: String,
    pub name: String,
    pub owner_email: String,
    pub created_at: DateTime<Utc>,
}

/// Orgs the old `personal_org` sign-up made: named `<name>'s org`, one member, who is its owner
/// and has it as their default organization. (Renamed or shared orgs no longer qualify.)
pub async fn list_personal_orgs<'e, E: PgExecutor<'e>>(
    exec: E,
    limit: i64,
) -> Result<Vec<PersonalOrg>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT o.id AS org_id, o.slug, o.name, u.email::text AS owner_email, o.created_at
         {PERSONAL_ORGS} ORDER BY o.created_at DESC, o.id DESC LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

pub async fn count_personal_orgs<'e, E: PgExecutor<'e>>(exec: E) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar(&format!("SELECT count(*) {PERSONAL_ORGS}"))
            .fetch_one(exec)
            .await?,
    )
}

const PERSONAL_ORGS: &str = "FROM organizations o
    JOIN org_members m ON m.org_id = o.id AND m.role = 'owner'
    JOIN users u ON u.id = m.user_id AND u.default_org_id = o.id
    WHERE o.name LIKE '%''s org'
      AND (SELECT count(*) FROM org_members x WHERE x.org_id = o.id) = 1";
