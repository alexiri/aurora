//! Organizations.

use super::{After, split};
use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OrgRow {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub app_posture: String,
    pub external_id: String,
    pub settings: serde_json::Value,
    pub created_at: DateTime<Utc>,
    /// Role of the requesting user.
    pub my_role: String,
}

/// `my_role` is the **effective** org role (the strongest of the direct role and a group's org
/// role, D-130(b), the `effective_org_members` view `/v1/me` reads), so the two never disagree;
/// a member who is not active has no effective row and keeps the direct one.
const SEL: &str = "o.id, o.slug, o.name, o.app_posture, o.external_id, o.settings, o.created_at, COALESCE(e.role, m.role) AS my_role";

/// The join the effective role comes from, after `org_members m`.
const EFFECTIVE: &str =
    "LEFT JOIN effective_org_members e ON e.org_id = m.org_id AND e.user_id = m.user_id";

pub async fn insert<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    slug: &str,
    name: &str,
    external_id: &str,
) -> Result<(), DbError> {
    sqlx::query("INSERT INTO organizations (id, slug, name, external_id) VALUES ($1,$2,$3,$4)")
        .bind(id)
        .bind(slug)
        .bind(name)
        .bind(external_id)
        .execute(exec)
        .await?;
    Ok(())
}

pub async fn slug_exists<'e, E: PgExecutor<'e>>(exec: E, slug: &str) -> Result<bool, DbError> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM organizations WHERE slug = $1)")
            .bind(slug)
            .fetch_one(exec)
            .await?,
    )
}

/// The org as seen by `user` (must be a member row).
pub async fn get<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    user: Uuid,
) -> Result<Option<OrgRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SEL} FROM organizations o JOIN org_members m ON m.org_id = o.id AND m.user_id = $2 {EFFECTIVE} WHERE o.id = $1"
    ))
    .bind(org)
    .bind(user)
    .fetch_optional(exec)
    .await?)
}

/// Orgs where `user` is an active member.
pub async fn list_for_user<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    after: After,
    fetch: i64,
) -> Result<Vec<OrgRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(&format!(
        "SELECT {SEL} FROM organizations o JOIN org_members m ON m.org_id = o.id AND m.user_id = $1 AND m.status = 'active'
         {EFFECTIVE}
         WHERE ($2::timestamptz IS NULL OR (o.created_at, o.id) < ($2, $3))
         ORDER BY o.created_at DESC, o.id DESC LIMIT $4"
    ))
    .bind(user)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

pub async fn update<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    name: Option<&str>,
    slug: Option<&str>,
    settings: Option<&serde_json::Value>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE organizations SET name = COALESCE($2, name), slug = COALESCE($3, slug),
         settings = COALESCE($4, settings), updated_at = now() WHERE id = $1",
    )
    .bind(org)
    .bind(name)
    .bind(slug)
    .bind(settings)
    .execute(exec)
    .await?;
    Ok(())
}

/// Delete an organization and its children in dependency order. Ledgers
/// (`audit_log`, `request_log`, `runtime_*`) are not FK-linked and remain.
/// M6: connector deprovisioning must be enqueued before calling this.
pub async fn delete_cascade(conn: &mut PgConnection, org: Uuid) -> Result<(), DbError> {
    // Workspaces first: cascades agents and every workspace-scoped row.
    for table in [
        "workspaces",
        "channel_integrations",
        "skills",
        "derived_rules",
        "app_connections",
        "app_oauth_clients",
        "app_instances",
        "secrets",
        "policy_generations",
        "api_keys",
    ] {
        sqlx::query(&format!("DELETE FROM {table} WHERE org_id = $1"))
            .bind(org)
            .execute(&mut *conn)
            .await?;
    }
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// `(id, slug, name)` by slug, regardless of caller (callers must authorize).
pub async fn get_by_slug<'e, E: PgExecutor<'e>>(
    exec: E,
    slug: &str,
) -> Result<Option<(Uuid, String, String)>, DbError> {
    Ok(
        sqlx::query_as("SELECT id, slug, name FROM organizations WHERE slug = $1")
            .bind(slug)
            .fetch_optional(exec)
            .await?,
    )
}

/// (FIX-050, C-630) Organizations with zero active owners, oldest first: the gateway refuses
/// every credential of such an org (06 §Credential authentication). `members` counts the
/// org's active members of any role (the people whose credentials are refused). One query;
/// the API's admin role may read `org_members`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OwnerlessRow {
    pub slug: String,
    pub members: i64,
    pub created_at: DateTime<Utc>,
}

pub async fn list_ownerless<'e, E: PgExecutor<'e>>(exec: E) -> Result<Vec<OwnerlessRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT o.slug, count(m.user_id) AS members, o.created_at
         FROM organizations o
         LEFT JOIN org_members m
           ON m.org_id = o.id AND m.status = 'active'
         GROUP BY o.id, o.slug, o.created_at
         HAVING count(m.user_id) FILTER (WHERE m.role = 'owner') = 0
         ORDER BY o.created_at, o.slug",
    )
    .fetch_all(exec)
    .await?)
}
