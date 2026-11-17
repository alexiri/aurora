//! Pending workspace access (phase 5, P5-UX-5, D-325, SEC-128, C-919, C-920): a workspace owner's grant to an
//! e-mail address that is not yet an active member of the organization. A row is applied at a sign-in and deleted; it
//! never makes anyone a member of the organization, and never carries `owner`.
//!
//! Every read here is scoped by the workspace (the owner's routes) or by the organization **and** the signed-in user's
//! own active membership (the sign-in lookup): nothing lists or searches people.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PendingRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub org_id: Uuid,
    pub email_normalized: String,
    pub role: String,
    pub invited_by: Option<Uuid>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

const COLS: &str =
    "id, workspace_id, org_id, email_normalized, role, invited_by, expires_at, created_at";

/// The outcome of [`add`].
#[derive(Debug)]
pub enum Added {
    /// Stored (a second add of the same address in the workspace refreshes its role and expiry).
    Row(PendingRow),
    /// The workspace already holds `max` unexpired rows for other addresses.
    Full,
}

/// Add (or refresh) one pending row. The workspace row is locked so two adds cannot both slip under `max`.
#[allow(clippy::too_many_arguments)]
pub async fn add(
    tx: &mut PgConnection,
    id: Uuid,
    workspace: Uuid,
    org: Uuid,
    email_normalized: &str,
    role: &str,
    invited_by: Uuid,
    ttl_secs: f64,
    max: i64,
) -> Result<Added, DbError> {
    sqlx::query("SELECT 1 FROM workspaces WHERE id = $1 FOR UPDATE")
        .bind(workspace)
        .execute(&mut *tx)
        .await?;
    // Expired rows do not count (the sweeper removes them; one that lapsed is replaced below).
    let others: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workspace_pending_access
         WHERE workspace_id = $1 AND expires_at > now() AND email_normalized <> $2",
    )
    .bind(workspace)
    .bind(email_normalized)
    .fetch_one(&mut *tx)
    .await?;
    if others >= max {
        return Ok(Added::Full);
    }
    let row = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO workspace_pending_access (id, workspace_id, org_id, email_normalized, role, invited_by, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, now() + make_interval(secs => $7))
         ON CONFLICT (workspace_id, email_normalized) DO UPDATE
           SET role = EXCLUDED.role, invited_by = EXCLUDED.invited_by, expires_at = EXCLUDED.expires_at, updated_at = now()
         RETURNING {COLS}"
    )))
    .bind(id)
    .bind(workspace)
    .bind(org)
    .bind(email_normalized)
    .bind(role)
    .bind(invited_by)
    .bind(ttl_secs)
    .fetch_one(&mut *tx)
    .await?;
    Ok(Added::Row(row))
}

/// The workspace's unexpired pending rows, oldest first (the owner's "Waiting for first sign-in" list).
pub async fn list<'e, E: PgExecutor<'e>>(
    exec: E,
    workspace: Uuid,
) -> Result<Vec<PendingRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM workspace_pending_access
         WHERE workspace_id = $1 AND expires_at > now() ORDER BY created_at, id"
    )))
    .bind(workspace)
    .fetch_all(exec)
    .await?)
}

/// Delete one row **of this workspace** (another workspace's id is never found); returns it.
pub async fn delete(
    tx: &mut PgConnection,
    workspace: Uuid,
    id: Uuid,
) -> Result<Option<PendingRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "DELETE FROM workspace_pending_access WHERE id = $1 AND workspace_id = $2 RETURNING {COLS}"
    )))
    .bind(id)
    .bind(workspace)
    .fetch_optional(&mut *tx)
    .await?)
}

/// The rows a sign-in applies (SEC-128): unexpired, addressed to one of the person's **verified** e-mails (already
/// normalized; exact equality, no prefix or wildcard), in an organization where `user` is an **active** member. Locked
/// so two sign-ins cannot apply a row twice.
pub async fn due_for_sign_in(
    tx: &mut PgConnection,
    user: Uuid,
    verified_emails: &[String],
) -> Result<Vec<PendingRow>, DbError> {
    if verified_emails.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM workspace_pending_access p
         WHERE p.email_normalized = ANY($1) AND p.expires_at > now()
           AND EXISTS (SELECT 1 FROM org_members m
                       WHERE m.org_id = p.org_id AND m.user_id = $2 AND m.status = 'active')
         ORDER BY p.created_at, p.id
         FOR UPDATE OF p"
    )))
    .bind(verified_emails)
    .bind(user)
    .fetch_all(&mut *tx)
    .await?)
}

/// Delete an applied row.
pub async fn delete_applied(tx: &mut PgConnection, id: Uuid) -> Result<(), DbError> {
    sqlx::query("DELETE FROM workspace_pending_access WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    Ok(())
}

/// The sweeper: remove rows past their expiry.
pub async fn sweep_expired<'e, E: PgExecutor<'e>>(exec: E) -> Result<u64, DbError> {
    Ok(
        sqlx::query("DELETE FROM workspace_pending_access WHERE expires_at <= now()")
            .execute(exec)
            .await?
            .rows_affected(),
    )
}

/// An active member of `org` whose **verified** e-mail equals `email_normalized` (exact; case-insensitive through the
/// column's type): the one person a by-e-mail add binds at once. `None` for anyone else.
pub async fn active_member_by_verified_email<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    email_normalized: &str,
) -> Result<Option<(Uuid, String)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT u.id, u.display_name FROM users u
         JOIN org_members m ON m.user_id = u.id AND m.org_id = $1 AND m.status = 'active'
         WHERE u.email = $2 AND u.email_verified_at IS NOT NULL AND u.disabled_at IS NULL",
    )
    .bind(org)
    .bind(email_normalized)
    .fetch_optional(exec)
    .await?)
}
