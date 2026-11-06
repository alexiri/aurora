//! Account-level queries: `/v1/me`, instance-admin user management, account deletion (BE-016).

use super::{After, split};
use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MembershipRow {
    pub org_id: Uuid,
    pub slug: String,
    pub name: String,
    pub role: String,
    pub status: String,
}

/// All of the user's memberships (active and suspended), oldest first.
pub async fn memberships<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
) -> Result<Vec<MembershipRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT o.id AS org_id, o.slug, o.name, m.role, m.status
         FROM org_members m JOIN organizations o ON o.id = m.org_id
         WHERE m.user_id = $1 ORDER BY m.created_at, o.id",
    )
    .bind(user)
    .fetch_all(exec)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UsableWorkspaceRow {
    pub org_id: Uuid,
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub role: String,
}

/// Workspaces the user can use through an *active* membership, with the effective role
/// (org owner/admin are implicit workspace owners).
pub async fn usable_workspaces<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
) -> Result<Vec<UsableWorkspaceRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT w.org_id, w.id, w.slug, w.name,
                CASE WHEN m.role IN ('owner','admin') THEN 'owner' ELSE a.role END AS role
         FROM workspaces w
         JOIN org_members m ON m.org_id = w.org_id AND m.user_id = $1 AND m.status = 'active'
         LEFT JOIN workspace_access a ON a.workspace_id = w.id AND a.user_id = $1
         WHERE m.role IN ('owner','admin') OR a.user_id IS NOT NULL
         ORDER BY w.created_at, w.id",
    )
    .bind(user)
    .fetch_all(exec)
    .await?)
}

pub async fn update_display_name<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    name: &str,
) -> Result<(), DbError> {
    sqlx::query("UPDATE users SET display_name = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(name)
        .execute(exec)
        .await?;
    Ok(())
}

pub async fn is_active_member<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    org: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM org_members WHERE user_id = $1 AND org_id = $2 AND status = 'active')",
    )
    .bind(user)
    .bind(org)
    .fetch_one(exec)
    .await?)
}

/// When the session was created (database clock age in seconds).
pub async fn session_age_secs<'e, E: PgExecutor<'e>>(
    exec: E,
    session: Uuid,
) -> Result<Option<f64>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM (now() - created_at))::float8 FROM sessions WHERE id = $1",
    )
    .bind(session)
    .fetch_optional(exec)
    .await?)
}

/// Orgs where `user` is the only active owner (rows locked; call inside a transaction).
pub async fn sole_owner_orgs(conn: &mut PgConnection, user: Uuid) -> Result<Vec<Uuid>, DbError> {
    let orgs: Vec<Uuid> = sqlx::query_scalar(
        "SELECT org_id FROM org_members WHERE user_id = $1 AND role = 'owner' AND status = 'active'
         ORDER BY org_id FOR UPDATE",
    )
    .bind(user)
    .fetch_all(&mut *conn)
    .await?;
    let mut sole = Vec::new();
    for org in orgs {
        let owners = super::members::lock_active_owners(&mut *conn, org).await?;
        if owners.iter().all(|o| *o == user) {
            sole.push(org);
        }
    }
    Ok(sole)
}

/// Delete the user row; memberships, bindings, sessions, keys, identities, tokens and
/// `direct` conversations go by FK cascade, creator/actor references are set to NULL.
pub async fn delete_user(conn: &mut PgConnection, user: Uuid) -> Result<(), DbError> {
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user)
        .execute(conn)
        .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AdminUserRow {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub email_verified_at: Option<DateTime<Utc>>,
    pub is_instance_admin: bool,
    pub disabled_at: Option<DateTime<Utc>>,
    pub last_login_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

const ADMIN_COLS: &str = "id, email::text AS email, display_name, email_verified_at, \
    is_instance_admin, disabled_at, last_login_at, created_at";

fn like_escape(q: &str) -> String {
    let mut s = String::with_capacity(q.len() + 2);
    s.push('%');
    for c in q.chars() {
        if matches!(c, '%' | '_' | '\\') {
            s.push('\\');
        }
        s.push(c);
    }
    s.push('%');
    s
}

pub async fn list_users<'e, E: PgExecutor<'e>>(
    exec: E,
    q: Option<&str>,
    after: After,
    fetch: i64,
) -> Result<Vec<AdminUserRow>, DbError> {
    let (t, id) = split(after);
    let pat = q.map(like_escape);
    Ok(sqlx::query_as(&format!(
        "SELECT {ADMIN_COLS} FROM users
         WHERE ($1::text IS NULL OR email::text ILIKE $1 OR display_name ILIKE $1)
         AND ($2::timestamptz IS NULL OR (created_at, id) < ($2, $3))
         ORDER BY created_at DESC, id DESC LIMIT $4"
    ))
    .bind(pat)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

pub async fn get_user<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<AdminUserRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("SELECT {ADMIN_COLS} FROM users WHERE id = $1"))
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}

/// Apply admin flags; `disabled`: `Some(true)` sets `disabled_at` (keeping an existing
/// value), `Some(false)` clears it. Returns the fresh row.
pub async fn update_admin_flags<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    is_instance_admin: Option<bool>,
    disabled: Option<bool>,
) -> Result<Option<AdminUserRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "UPDATE users SET
            is_instance_admin = COALESCE($2, is_instance_admin),
            disabled_at = CASE WHEN $3::bool IS NULL THEN disabled_at
                               WHEN $3 THEN COALESCE(disabled_at, now()) ELSE NULL END,
            updated_at = now()
         WHERE id = $1 RETURNING {ADMIN_COLS}"
    ))
    .bind(id)
    .bind(is_instance_admin)
    .bind(disabled)
    .fetch_optional(exec)
    .await?)
}

/// Clear the instance-admin flag by email; returns whether a user matched.
pub async fn revoke_instance_admin_by_email<'e, E: PgExecutor<'e>>(
    exec: E,
    email: &str,
) -> Result<Option<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE users SET is_instance_admin = false, updated_at = now()
         WHERE email = $1::citext RETURNING id",
    )
    .bind(email.trim())
    .fetch_optional(exec)
    .await?)
}

/// Insert a verified, password-less instance admin, or promote/verify the existing user.
pub async fn upsert_bootstrap_admin<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    email: &str,
    display_name: &str,
) -> Result<Uuid, DbError> {
    Ok(sqlx::query_scalar(
        "INSERT INTO users (id, email, display_name, email_verified_at, is_instance_admin)
         VALUES ($1, $2::citext, $3, now(), true)
         ON CONFLICT (email) DO UPDATE SET is_instance_admin = true,
            email_verified_at = COALESCE(users.email_verified_at, now()), updated_at = now()
         RETURNING id",
    )
    .bind(id)
    .bind(email)
    .bind(display_name)
    .fetch_one(exec)
    .await?)
}
