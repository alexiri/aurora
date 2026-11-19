//! App setup requests (P5-POLISH-4, D-386, C-1030, SEC-135): a person asks the organization's owners
//! and admins to make an app ready. A request is a record and a notice, never a grant: nothing here
//! touches availability, OAuth clients or access. No free text is stored.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RequestRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub app_id: String,
    pub status: String,
    pub needs: serde_json::Value,
    pub unavailable: bool,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub closed_by: Option<Uuid>,
}

const SELECT: &str = "SELECT id, org_id, app_id, status, needs, unavailable, created_at, closed_at, closed_by \
    FROM app_setup_requests";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AskerRow {
    pub request_id: Uuid,
    pub user_id: Uuid,
    pub display_name: String,
    pub workspace_id: Uuid,
    pub asked_at: DateTime<Utc>,
}

/// The open request of an organization for an app, locked for the rest of the transaction.
pub async fn open_for_update<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    app: &str,
) -> Result<Option<RequestRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE org_id = $1 AND app_id = $2 AND status = 'open' FOR UPDATE"
    )))
    .bind(org)
    .bind(app)
    .fetch_optional(exec)
    .await?)
}

pub async fn get<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<RequestRow>, DbError> {
    Ok(
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE id = $1")))
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}

/// Opens a request; false when another one is already open for `(org, app)` (a concurrent ask won).
pub async fn insert<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    org: Uuid,
    app: &str,
    needs: &serde_json::Value,
    unavailable: bool,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "INSERT INTO app_setup_requests (id, org_id, app_id, needs, unavailable) \
         VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(org)
    .bind(app)
    .bind(needs)
    .bind(unavailable)
    .execute(exec)
    .await?
    .rows_affected()
        == 1)
}

pub async fn set_needs<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    needs: &serde_json::Value,
    unavailable: bool,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE app_setup_requests SET needs = $2, unavailable = $3, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(needs)
    .bind(unavailable)
    .execute(exec)
    .await?;
    Ok(())
}

/// Adds the person's ask (or moves it to the workspace they ask from); true when it is new.
pub async fn add_asker<'e, E: PgExecutor<'e>>(
    exec: E,
    request: Uuid,
    user: Uuid,
    workspace: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "INSERT INTO app_setup_request_askers (request_id, user_id, workspace_id) VALUES ($1, $2, $3) \
         ON CONFLICT (request_id, user_id) DO UPDATE SET workspace_id = EXCLUDED.workspace_id \
         RETURNING (xmax = 0)",
    )
    .bind(request)
    .bind(user)
    .bind(workspace)
    .fetch_one(exec)
    .await?)
}

/// Removes the person's ask; returns how many askers remain (`None` when they had not asked).
pub async fn remove_asker<'e, E: PgExecutor<'e>>(
    exec: E,
    request: Uuid,
    user: Uuid,
) -> Result<Option<i64>, DbError> {
    Ok(sqlx::query_scalar(
        "WITH d AS (DELETE FROM app_setup_request_askers WHERE request_id = $1 AND user_id = $2 RETURNING 1) \
         SELECT CASE WHEN EXISTS (SELECT 1 FROM d) \
             THEN (SELECT count(*) FROM app_setup_request_askers WHERE request_id = $1 AND user_id <> $2) END",
    )
    .bind(request)
    .bind(user)
    .fetch_one(exec)
    .await?)
}

/// Closes an open request; false when it was not open.
pub async fn close<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    status: &str,
    closed_by: Option<Uuid>,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE app_setup_requests SET status = $2, closed_at = now(), closed_by = $3, updated_at = now() \
         WHERE id = $1 AND status = 'open'",
    )
    .bind(id)
    .bind(status)
    .bind(closed_by)
    .execute(exec)
    .await?
    .rows_affected()
        == 1)
}

pub async fn askers<'e, E: PgExecutor<'e>>(
    exec: E,
    requests: &[Uuid],
) -> Result<Vec<AskerRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT a.request_id, a.user_id, u.display_name, a.workspace_id, a.asked_at \
         FROM app_setup_request_askers a JOIN users u ON u.id = a.user_id \
         WHERE a.request_id = ANY($1) ORDER BY a.asked_at, a.user_id",
    )
    .bind(requests)
    .fetch_all(exec)
    .await?)
}

/// An organization's requests with `status` (`None`: any), the most recently asked first.
pub async fn list_org<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    status: Option<&str>,
) -> Result<Vec<RequestRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE org_id = $1 AND ($2::text IS NULL OR status = $2) ORDER BY updated_at DESC, id LIMIT 500"
    )))
    .bind(org)
    .bind(status)
    .fetch_all(exec)
    .await?)
}

/// Every open request, of one organization or of all (the periodic re-evaluation).
pub async fn open_requests<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Option<Uuid>,
) -> Result<Vec<RequestRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE status = 'open' AND ($1::uuid IS NULL OR org_id = $1) ORDER BY created_at LIMIT 2000"
    )))
    .bind(org)
    .fetch_all(exec)
    .await?)
}

/// `(app_id, askers, asked_by_user)` of the open requests of an organization (the catalogue's `requested`).
pub async fn open_counts<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    user: Uuid,
) -> Result<Vec<(String, i64, bool)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT r.app_id, count(a.user_id), COALESCE(bool_or(a.user_id = $2), false) \
         FROM app_setup_requests r JOIN app_setup_request_askers a ON a.request_id = r.id \
         WHERE r.org_id = $1 AND r.status = 'open' GROUP BY r.app_id",
    )
    .bind(org)
    .bind(user)
    .fetch_all(exec)
    .await?)
}

/// Active owners and admins of an organization: `(user_id, email)`.
pub async fn org_admins<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
) -> Result<Vec<(Uuid, String)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT u.id, u.email::text FROM effective_org_members e \
         JOIN users u ON u.id = e.user_id AND u.disabled_at IS NULL \
         WHERE e.org_id = $1 AND e.role IN ('owner', 'admin') ORDER BY u.id",
    )
    .bind(org)
    .fetch_all(exec)
    .await?)
}

/// Whether the person is an active member of the organization.
pub async fn asker_name<'e, E: PgExecutor<'e>>(exec: E, user: Uuid) -> Result<String, DbError> {
    Ok(
        sqlx::query_scalar("SELECT display_name FROM users WHERE id = $1")
            .bind(user)
            .fetch_one(exec)
            .await?,
    )
}

// ---- notices -----------------------------------------------------------------------------

/// Records a notice for one recipient; a replay inserts nothing.
pub async fn insert_notice<'e, E: PgExecutor<'e>>(
    exec: E,
    request: Uuid,
    user: Uuid,
    kind: &str,
    title: &str,
    detail: &str,
    href: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "INSERT INTO app_setup_request_notices (request_id, user_id, kind, title, detail, href) \
         VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
    )
    .bind(request)
    .bind(user)
    .bind(kind)
    .bind(title)
    .bind(detail)
    .bind(href)
    .execute(exec)
    .await?
    .rows_affected()
        == 1)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Claimed {
    pub request_id: Uuid,
    pub user_id: Uuid,
    pub kind: String,
    pub title: String,
    pub detail: String,
    pub href: String,
    pub email: String,
    /// The recipient is still an enabled user.
    pub active: bool,
}

/// Claims up to `limit` notices whose mail half is open (claim, then send: a crash loses one mail,
/// never doubles it), skipping rows another replica holds.
pub async fn claim_mail<'e, E: PgExecutor<'e>>(
    exec: E,
    limit: i64,
) -> Result<Vec<Claimed>, DbError> {
    Ok(sqlx::query_as(
        "WITH picked AS (SELECT request_id, user_id, kind FROM app_setup_request_notices \
            WHERE mailed_at IS NULL ORDER BY created_at LIMIT $1 FOR UPDATE SKIP LOCKED) \
         UPDATE app_setup_request_notices n SET mailed_at = now() \
         FROM picked p JOIN users u ON u.id = p.user_id \
         WHERE n.request_id = p.request_id AND n.user_id = p.user_id AND n.kind = p.kind \
         RETURNING n.request_id, n.user_id, n.kind, n.title, n.detail, n.href, u.email::text AS email, \
            (u.disabled_at IS NULL) AS active",
    )
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// The same for the Web Push half; only notices younger than `max_age_secs` (a push for a long-closed
/// request is noise).
pub async fn claim_push<'e, E: PgExecutor<'e>>(
    exec: E,
    max_age_secs: f64,
    limit: i64,
) -> Result<Vec<Claimed>, DbError> {
    Ok(sqlx::query_as(
        "WITH picked AS (SELECT request_id, user_id, kind FROM app_setup_request_notices \
            WHERE pushed_at IS NULL AND created_at > now() - make_interval(secs => $1) \
            ORDER BY created_at LIMIT $2 FOR UPDATE SKIP LOCKED) \
         UPDATE app_setup_request_notices n SET pushed_at = now() \
         FROM picked p JOIN users u ON u.id = p.user_id \
         WHERE n.request_id = p.request_id AND n.user_id = p.user_id AND n.kind = p.kind \
         RETURNING n.request_id, n.user_id, n.kind, n.title, n.detail, n.href, u.email::text AS email, \
            (u.disabled_at IS NULL) AS active",
    )
    .bind(max_age_secs)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// Retention: closed requests (and with them their askers and notices) after `days`.
pub async fn purge_closed<'e, E: PgExecutor<'e>>(exec: E, days: i32) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM app_setup_requests WHERE status <> 'open' \
         AND closed_at < now() - make_interval(days => $1)",
    )
    .bind(days)
    .execute(exec)
    .await?
    .rows_affected())
}
