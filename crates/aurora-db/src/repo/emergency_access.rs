//! Emergency read access (phase 5, FIX-236, D-478, C-1164, SEC-154): the
//! `workspace_emergency_access` rows. A grant is **active** while
//! `ended_at IS NULL AND expires_at > now()` — every check evaluates that itself (`authz::
//! emergency_reader`), never a cached value, and never only through the expiry sweep. This
//! module owns open, end, list, expire, end-on-lost-ownership and the 400-day retention prune;
//! the audit rows the API layer writes around these calls are the security-critical record.
//!
//! Every list is scoped by the workspace or organization and returns metadata only (never
//! conversation content).

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EmergencyRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub user_id: Uuid,
    pub reason: String,
    pub opened_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub ended_by: Option<Uuid>,
    pub end_reason: Option<String>,
}

const COLS: &str = "id, org_id, workspace_id, user_id, reason, opened_at, expires_at, ended_at, \
                    ended_by, end_reason";
/// `COLS` prefixed with the row alias `e.`, for the joined list queries.
const COLS_LIST: &str = "e.id, e.org_id, e.workspace_id, e.user_id, e.reason, e.opened_at, \
                         e.expires_at, e.ended_at, e.ended_by, e.end_reason";

/// Why a grant ended (`workspace_emergency_access.end_reason`).
pub const END_ENDED: &str = "ended";
pub const END_EXPIRED: &str = "expired";
pub const END_ACCESS_LOST: &str = "access_lost";

/// Insert one grant. The caller has already refused a second active one (409
/// `emergency_access_active`); the partial unique index backs that up.
#[allow(clippy::too_many_arguments)]
/// (Review LOW-10, verify MEDIUM-1) End the caller's own expired-but-unswept grants on the
/// workspace, inside the open transaction: the minute sweep (`expire_due`) runs on its own
/// schedule, so for up to a minute after `expires_at` the stale row still holds the partial
/// unique index and a reopen would fail with a generic conflict. This gives it exactly the
/// sweep's shape (`ended_at = expires_at`, `end_reason = 'expired'`) and returns the ended rows
/// so the caller writes the sweep's critical `workspace.emergency_access.expire` audit rows in
/// the same transaction — the log must show open → expire → open even when the reopen beats
/// the sweep.
pub async fn expire_own_stale(
    tx: &mut PgConnection,
    ws: Uuid,
    user: Uuid,
) -> Result<Vec<EmergencyRow>, DbError> {
    Ok(
        sqlx::query_as::<_, EmergencyRow>(sqlx::AssertSqlSafe(format!(
            "UPDATE workspace_emergency_access \
         SET ended_at = expires_at, ended_by = NULL, end_reason = '{END_EXPIRED}' \
         WHERE workspace_id = $1 AND user_id = $2 \
           AND ended_at IS NULL AND expires_at <= now() RETURNING {COLS}"
        )))
        .bind(ws)
        .bind(user)
        .fetch_all(&mut *tx)
        .await?,
    )
}

pub async fn open(
    tx: &mut PgConnection,
    id: Uuid,
    org: Uuid,
    ws: Uuid,
    user: Uuid,
    reason: &str,
    expires_at: DateTime<Utc>,
) -> Result<EmergencyRow, DbError> {
    sqlx::query_as::<_, EmergencyRow>(sqlx::AssertSqlSafe(format!(
        "INSERT INTO workspace_emergency_access \
           (id, org_id, workspace_id, user_id, reason, opened_at, expires_at) \
         VALUES ($1, $2, $3, $4, $5, now(), $6) RETURNING {COLS}"
    )))
    .bind(id)
    .bind(org)
    .bind(ws)
    .bind(user)
    .bind(reason)
    .bind(expires_at)
    .fetch_one(&mut *tx)
    .await
    .map_err(Into::into)
}

/// The caller's own active grant on the workspace, if any (the `emergency_access_active`
/// conflict and the `GET` list's `active` flag).
pub async fn active_of<'e, E: PgExecutor<'e>>(
    exec: E,
    ws: Uuid,
    user: Uuid,
) -> Result<Option<EmergencyRow>, DbError> {
    Ok(
        sqlx::query_as::<_, EmergencyRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {COLS} FROM workspace_emergency_access \
         WHERE workspace_id = $1 AND user_id = $2 \
           AND ended_at IS NULL AND expires_at > now()"
        )))
        .bind(ws)
        .bind(user)
        .fetch_optional(exec)
        .await?,
    )
}

/// One grant row by id, for the end route's answer.
pub async fn get<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<EmergencyRow>, DbError> {
    Ok(
        sqlx::query_as::<_, EmergencyRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {COLS} FROM workspace_emergency_access WHERE id = $1"
        )))
        .bind(id)
        .fetch_optional(exec)
        .await?,
    )
}

/// Lock a grant row (`FOR UPDATE`) so two enders cannot race the write.
pub async fn lock(tx: &mut PgConnection, id: Uuid) -> Result<Option<EmergencyRow>, DbError> {
    Ok(
        sqlx::query_as::<_, EmergencyRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {COLS} FROM workspace_emergency_access WHERE id = $1 FOR UPDATE"
        )))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?,
    )
}

/// End one grant (early end, expiry or access-lost). No-op when it already ended; the caller
/// answers the row either way (idempotent). Returns whether this call ended it.
pub async fn end(
    tx: &mut PgConnection,
    id: Uuid,
    ended_by: Option<Uuid>,
    end_reason: &str,
) -> Result<bool, DbError> {
    let n = sqlx::query(
        "UPDATE workspace_emergency_access \
         SET ended_at = now(), ended_by = $2, end_reason = $3 \
         WHERE id = $1 AND ended_at IS NULL",
    )
    .bind(id)
    .bind(ended_by)
    .bind(end_reason)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    Ok(n > 0)
}

/// The workspace's grants, newest first (C-1164 `GET /v1/workspaces/{ws}/emergency-access`),
/// with the workspace's and the opener's display names (a person's e-mail is never a label).
/// Keyset paging over `(opened_at DESC, id DESC)`; the API layer fetches `limit + 1` rows.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ListRow {
    #[sqlx(flatten)]
    pub row: EmergencyRow,
    pub workspace_name: String,
    pub by_display_name: String,
}

pub async fn list_for_workspace<'e, E: PgExecutor<'e>>(
    exec: E,
    ws: Uuid,
    limit: i64,
    before: Option<(DateTime<Utc>, Uuid)>,
) -> Result<Vec<ListRow>, DbError> {
    Ok(sqlx::query_as::<_, ListRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS_LIST}, w.name AS workspace_name, u.display_name AS by_display_name \
         FROM workspace_emergency_access e \
         JOIN workspaces w ON w.id = e.workspace_id \
         JOIN users u ON u.id = e.user_id \
         WHERE e.workspace_id = $1 \
           AND ($2::timestamptz IS NULL OR (e.opened_at, e.id) < ($2, $3::uuid)) \
         ORDER BY e.opened_at DESC, e.id DESC LIMIT $4"
    )))
    .bind(ws)
    .bind(before.map(|(t, _)| t))
    .bind(before.map(|(_, id)| id))
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// The organization's grants, newest first (`GET /v1/orgs/{org}/emergency-access`).
pub async fn list_for_org<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    limit: i64,
    before: Option<(DateTime<Utc>, Uuid)>,
) -> Result<Vec<ListRow>, DbError> {
    Ok(sqlx::query_as::<_, ListRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS_LIST}, w.name AS workspace_name, u.display_name AS by_display_name \
         FROM workspace_emergency_access e \
         JOIN workspaces w ON w.id = e.workspace_id \
         JOIN users u ON u.id = e.user_id \
         WHERE e.org_id = $1 \
           AND ($2::timestamptz IS NULL OR (e.opened_at, e.id) < ($2, $3::uuid)) \
         ORDER BY e.opened_at DESC, e.id DESC LIMIT $4"
    )))
    .bind(org)
    .bind(before.map(|(t, _)| t))
    .bind(before.map(|(_, id)| id))
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// The banner's `audience.emergency` (C-1165, review HIGH-1): the newest **active** grant of the
/// workspace whenever any is active (an owner's open access may never be hidden behind another
/// owner's newer, already-ended one), else the newest grant that ended within the last 7 days —
/// with the opener's display name (never their e-mail). Also returns how many **other** grants
/// are active right now, so two simultaneous openers are both announced. `None` when the
/// workspace has nothing to show.
pub async fn latest_banner<'e, E: PgExecutor<'e> + Copy>(
    exec: E,
    ws: Uuid,
) -> Result<Option<(ListRow, i64)>, DbError> {
    let row = sqlx::query_as::<_, ListRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS_LIST}, w.name AS workspace_name, u.display_name AS by_display_name \
         FROM workspace_emergency_access e \
         JOIN workspaces w ON w.id = e.workspace_id \
         JOIN users u ON u.id = e.user_id \
         WHERE e.workspace_id = $1 \
           AND (e.ended_at IS NULL OR e.ended_at > now() - interval '7 days') \
         ORDER BY (e.ended_at IS NULL AND e.expires_at > now()) DESC, e.opened_at DESC, e.id DESC \
         LIMIT 1"
    )))
    .bind(ws)
    .fetch_optional(exec)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    // The other grants active right now (all of them when the shown row is the ended fallback:
    // there are none then, or the active-first ordering would have shown one).
    let more: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workspace_emergency_access \
         WHERE workspace_id = $1 AND ended_at IS NULL AND expires_at > now() AND id <> $2",
    )
    .bind(ws)
    .bind(row.row.id)
    .fetch_one(exec)
    .await?;
    Ok(Some((row, more)))
}

/// The minute expiry job (C-1164): every grant whose `expires_at` passed while still open gets
/// `ended_at = expires_at`, `end_reason = 'expired'`, `ended_by = NULL`. Returns the rows it
/// ended, so the API layer writes one `workspace.emergency_access.expire` audit row each **in
/// the same transaction** (use never waits for this job; `emergency_reader` already refuses an
/// expired grant by itself).
pub async fn expire_due(tx: &mut PgConnection) -> Result<Vec<EmergencyRow>, DbError> {
    Ok(
        sqlx::query_as::<_, EmergencyRow>(sqlx::AssertSqlSafe(format!(
            "UPDATE workspace_emergency_access \
         SET ended_at = expires_at, ended_by = NULL, end_reason = '{END_EXPIRED}' \
         WHERE ended_at IS NULL AND expires_at <= now() \
         RETURNING {COLS}"
        )))
        .fetch_all(&mut *tx)
        .await?,
    )
}

/// End every active grant `user` opened **in `org`**, with `access_lost` (C-1164: the opener
/// stopped being an active org owner there; review MEDIUM-5: scoped by org — losing ownership
/// in one organization never ends grants in another). Returns the rows it ended, so the caller
/// writes the audit rows in the **same transaction** as the membership change.
pub async fn end_lost_for_user(
    tx: &mut PgConnection,
    org: Uuid,
    user: Uuid,
) -> Result<Vec<EmergencyRow>, DbError> {
    Ok(
        sqlx::query_as::<_, EmergencyRow>(sqlx::AssertSqlSafe(format!(
            "UPDATE workspace_emergency_access \
         SET ended_at = now(), ended_by = NULL, end_reason = '{END_ACCESS_LOST}' \
         WHERE org_id = $1 AND user_id = $2 AND ended_at IS NULL \
         RETURNING {COLS}"
        )))
        .bind(org)
        .bind(user)
        .fetch_all(&mut *tx)
        .await?,
    )
}

/// Delete grants older than `age_days` (the 400-day retention, nightly sweep).
pub async fn prune(pool: &crate::PgPool, age_days: u32) -> Result<u64, DbError> {
    let n = sqlx::query(
        "DELETE FROM workspace_emergency_access WHERE opened_at < now() - make_interval(days => $1)",
    )
    .bind(i32::try_from(age_days).unwrap_or(i32::MAX))
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n)
}
