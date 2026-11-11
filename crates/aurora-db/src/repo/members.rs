//! Organization members.

use super::{After, split};
use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MemberRow {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    /// (P4-IDP-1) The IdP role value that gives (part of) this role; `None` for manual roles.
    pub idp_value: Option<String>,
}

const SEL: &str = "m.user_id, u.email::text AS email, u.display_name, m.role, m.status, m.created_at, \
    substring(m.idp_source from '^idp:[^:]*:(.*)$') AS idp_value";

/// The directory-sync projection of one member (P4-GRP-2): the `scim_users` link state and the
/// IdP's `externalId` (`None` without a link; the Members page shows a "Directory" badge).
const SCIM_SEL: &str = "s.state AS scim_state, s.external_id AS scim_external_id";

/// A member with their SCIM link projection (the tenancy Members list; `scim_state` is `None`
/// when the org has no link for this user).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MemberWithScimRow {
    #[sqlx(flatten)]
    pub member: MemberRow,
    pub scim_state: Option<String>,
    pub scim_external_id: Option<String>,
}

/// List an org's members with their SCIM link projection (one extra `LEFT JOIN`).
pub async fn list_with_scim<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    after: super::After,
    limit: i64,
    status: Option<&str>,
) -> Result<Vec<MemberWithScimRow>, DbError> {
    let (after_ts, after_id) = super::split(after);
    Ok(sqlx::query_as(&format!(
        "SELECT {SEL}, {SCIM_SEL} FROM org_members m \
         JOIN users u ON u.id = m.user_id \
         LEFT JOIN scim_users s ON s.org_id = m.org_id AND s.user_id = m.user_id \
         WHERE m.org_id = $1 \
           AND ($2::text IS NULL OR m.status = $2) \
           AND ($3::timestamptz IS NULL OR (m.created_at, m.user_id) < ($3, $4)) \
         ORDER BY m.created_at DESC, m.user_id DESC LIMIT $5"
    ))
    .bind(org)
    .bind(status)
    .bind(after_ts)
    .bind(after_id)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

pub async fn insert<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    user: Uuid,
    role: &str,
) -> Result<(), DbError> {
    sqlx::query("INSERT INTO org_members (org_id, user_id, role) VALUES ($1,$2,$3)")
        .bind(org)
        .bind(user)
        .bind(role)
        .execute(exec)
        .await?;
    Ok(())
}

pub async fn get<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    user: Uuid,
) -> Result<Option<MemberRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SEL} FROM org_members m JOIN users u ON u.id = m.user_id WHERE m.org_id = $1 AND m.user_id = $2"
    ))
    .bind(org)
    .bind(user)
    .fetch_optional(exec)
    .await?)
}

pub async fn list<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    status: Option<&str>,
    after: After,
    fetch: i64,
) -> Result<Vec<MemberRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(&format!(
        "SELECT {SEL} FROM org_members m JOIN users u ON u.id = m.user_id
         WHERE m.org_id = $1 AND ($2::text IS NULL OR m.status = $2)
         AND ($3::timestamptz IS NULL OR (m.created_at, m.user_id) < ($3, $4))
         ORDER BY m.created_at DESC, m.user_id DESC LIMIT $5"
    ))
    .bind(org)
    .bind(status)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

/// Lock the active owner rows of `org` and return their user ids.
pub async fn lock_active_owners(conn: &mut PgConnection, org: Uuid) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT user_id FROM org_members WHERE org_id = $1 AND role = 'owner' AND status = 'active' FOR UPDATE",
    )
    .bind(org)
    .fetch_all(conn)
    .await?)
}

pub async fn update<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    user: Uuid,
    role: Option<&str>,
    status: Option<&str>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE org_members SET role = COALESCE($3, role), status = COALESCE($4, status), updated_at = now()
         WHERE org_id = $1 AND user_id = $2",
    )
    .bind(org)
    .bind(user)
    .bind(role)
    .bind(status)
    .execute(exec)
    .await?;
    Ok(())
}

/// Remove the membership and the user's bindings in the org's workspaces.
pub async fn remove(conn: &mut PgConnection, org: Uuid, user: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "DELETE FROM workspace_access WHERE user_id = $2
         AND workspace_id IN (SELECT id FROM workspaces WHERE org_id = $1)",
    )
    .bind(org)
    .bind(user)
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM org_members WHERE org_id = $1 AND user_id = $2")
        .bind(org)
        .bind(user)
        .execute(&mut *conn)
        .await?;
    Ok(())
}
