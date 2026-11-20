//! App setup requests (P5-POLISH-4, D-386, C-1030, SEC-135): a person asks the organization's owners
//! and admins to make an app ready. A request is a record and a notice, never a grant: nothing here
//! touches availability, OAuth clients or access. (P5-REFINE-8, C-1081, SEC-135 amended) The only free
//! text is the asker's `reason` and the decliner's `closed_message`, sanitized and bounded by the API.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RequestRow {
    pub id: Uuid,
    pub org_id: Uuid,
    /// `app` | `chat_platform` | `ai_key` (P5-APIGAP-2, C-1047).
    pub subject: String,
    /// The app id; the connector type (or `""`) for a chat platform; `""` for an AI key.
    pub app_id: String,
    pub status: String,
    pub needs: serde_json::Value,
    pub unavailable: bool,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub closed_by: Option<Uuid>,
    /// (C-1081) The decliner's message to the askers.
    pub closed_message: Option<String>,
    /// (C-1081) `org_admin` | `instance_admin` | `system`.
    pub closed_by_role: Option<String>,
    /// The decliner's display name (never an e-mail).
    pub closed_by_name: Option<String>,
    /// (C-1080) A connector of the asked type (any, for none) is `active` on the instance: the chat
    /// platform's level rule. Computed in the query (always false for the other subjects).
    pub connector_active: bool,
}

const SELECT: &str = "SELECT r.id, r.org_id, r.subject, r.app_id, r.status, r.needs, r.unavailable, \
    r.created_at, r.closed_at, r.closed_by, r.closed_message, r.closed_by_role, \
    cu.display_name AS closed_by_name, \
    (r.subject = 'chat_platform' AND EXISTS (SELECT 1 FROM connectors k WHERE k.status = 'active' \
        AND (r.app_id = '' OR k.id = r.app_id OR k.type_id = r.app_id))) AS connector_active \
    FROM app_setup_requests r LEFT JOIN users cu ON cu.id = r.closed_by";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AskerRow {
    pub request_id: Uuid,
    pub user_id: Uuid,
    pub display_name: String,
    pub workspace_id: Uuid,
    pub asked_at: DateTime<Utc>,
    /// (C-1081) The asker's own reason.
    pub reason: Option<String>,
}

/// The open request of an organization for an app, locked for the rest of the transaction.
pub async fn open_for_update<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    subject: &str,
    app: &str,
) -> Result<Option<RequestRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE r.org_id = $1 AND r.subject = $2 AND r.app_id = $3 AND r.status = 'open' \
         FOR UPDATE OF r"
    )))
    .bind(org)
    .bind(subject)
    .bind(app)
    .fetch_optional(exec)
    .await?)
}

pub async fn get<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<RequestRow>, DbError> {
    Ok(
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE r.id = $1")))
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
    subject: &str,
    app: &str,
    needs: &serde_json::Value,
    unavailable: bool,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "INSERT INTO app_setup_requests (id, org_id, subject, app_id, needs, unavailable) \
         VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(org)
    .bind(subject)
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

/// Adds the person's ask (or moves it to the workspace they ask from); true when it is new. (C-1081) A
/// `reason` replaces the person's stored one; `None` keeps it.
pub async fn add_asker<'e, E: PgExecutor<'e>>(
    exec: E,
    request: Uuid,
    user: Uuid,
    workspace: Uuid,
    reason: Option<&str>,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "INSERT INTO app_setup_request_askers (request_id, user_id, workspace_id, reason) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (request_id, user_id) DO UPDATE SET workspace_id = EXCLUDED.workspace_id, \
             reason = COALESCE(EXCLUDED.reason, app_setup_request_askers.reason) \
         RETURNING (xmax = 0)",
    )
    .bind(request)
    .bind(user)
    .bind(workspace)
    .bind(reason)
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

/// How a request closes (C-1081): its status, and for a decline who, as which role, with what message.
#[derive(Debug, Clone, Copy)]
pub struct Closing<'a> {
    pub status: &'a str,
    pub by: Option<Uuid>,
    /// `org_admin` | `instance_admin` | `system`; `None` for `withdrawn`.
    pub role: Option<&'a str>,
    pub message: Option<&'a str>,
}

/// Closes an open request; false when it was not open.
pub async fn close<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    how: Closing<'_>,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE app_setup_requests SET status = $2, closed_at = now(), closed_by = $3, \
             closed_by_role = $4, closed_message = $5, updated_at = now() \
         WHERE id = $1 AND status = 'open'",
    )
    .bind(id)
    .bind(how.status)
    .bind(how.by)
    .bind(how.role)
    .bind(how.message)
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
        "SELECT a.request_id, a.user_id, u.display_name, a.workspace_id, a.asked_at, a.reason \
         FROM app_setup_request_askers a JOIN users u ON u.id = a.user_id \
         WHERE a.request_id = ANY($1) ORDER BY a.asked_at, a.user_id",
    )
    .bind(requests)
    .fetch_all(exec)
    .await?)
}

/// (C-1080) The people a request was resolved for (they received its `resolved` notice; their ask is
/// gone): `(request_id, user_id, display_name, notice time)`, oldest first.
pub async fn resolved_for<'e, E: PgExecutor<'e>>(
    exec: E,
    requests: &[Uuid],
) -> Result<Vec<(Uuid, Uuid, String, DateTime<Utc>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n.request_id, n.user_id, u.display_name, n.created_at \
         FROM app_setup_request_notices n JOIN users u ON u.id = n.user_id \
         WHERE n.request_id = ANY($1) AND n.kind = 'resolved' ORDER BY n.created_at, n.user_id",
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
        "{SELECT} WHERE r.org_id = $1 AND ($2::text IS NULL OR r.status = $2) \
         ORDER BY r.updated_at DESC, r.id LIMIT 500"
    )))
    .bind(org)
    .bind(status)
    .fetch_all(exec)
    .await?)
}

/// (C-1081) Requests closed `resolved` or `declined` in the last `days`, of one organization or of all,
/// newest first.
pub async fn closed_within<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Option<Uuid>,
    days: i32,
    limit: i64,
) -> Result<Vec<RequestRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE r.status IN ('resolved', 'declined') AND ($1::uuid IS NULL OR r.org_id = $1) \
         AND r.closed_at >= now() - make_interval(days => $2) \
         ORDER BY r.closed_at DESC, r.id LIMIT $3"
    )))
    .bind(org)
    .bind(days)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// (C-1081) The person's own history in an organization: the requests resolved for them (by their
/// `resolved` notice, the request may still be open for others) and the ones declined with their ask on
/// them, in the last `days`. `done_at` is when it ended for them; newest first.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MineDone {
    #[sqlx(flatten)]
    pub row: RequestRow,
    pub done_at: DateTime<Utc>,
    /// It ended because it was resolved for this person (not declined).
    pub resolved_for_me: bool,
}

pub async fn history_of_asker<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    user: Uuid,
    days: i32,
    limit: i64,
) -> Result<Vec<MineDone>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT q.*, d.done_at, d.resolved_for_me FROM ({SELECT}) q \
         JOIN (SELECT n.request_id, n.created_at AS done_at, true AS resolved_for_me \
                 FROM app_setup_request_notices n WHERE n.user_id = $2 AND n.kind = 'resolved' \
               UNION ALL \
               SELECT x.id, x.closed_at, false FROM app_setup_requests x \
                 JOIN app_setup_request_askers a ON a.request_id = x.id AND a.user_id = $2 \
                 WHERE x.status = 'declined' AND x.org_id = $1) d ON d.request_id = q.id \
         WHERE q.org_id = $1 AND d.done_at >= now() - make_interval(days => $3) \
         ORDER BY d.done_at DESC, q.id LIMIT $4"
    )))
    .bind(org)
    .bind(user)
    .bind(days)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// Every open request, of one organization or of all (the periodic re-evaluation, the feeds and the
/// counts of C-1080), oldest first.
pub async fn open_requests<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Option<Uuid>,
) -> Result<Vec<RequestRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE r.status = 'open' AND ($1::uuid IS NULL OR r.org_id = $1) \
         ORDER BY r.created_at, r.id LIMIT 2000"
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
         WHERE r.org_id = $1 AND r.status = 'open' AND r.subject = 'app' GROUP BY r.app_id",
    )
    .bind(org)
    .bind(user)
    .fetch_all(exec)
    .await?)
}

/// The open requests of an organization the person has asked (all subjects), the latest first.
pub async fn open_of_asker<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    user: Uuid,
) -> Result<Vec<RequestRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE r.org_id = $1 AND r.status = 'open' \
         AND r.id IN (SELECT request_id FROM app_setup_request_askers WHERE user_id = $2) \
         ORDER BY r.updated_at DESC, r.id LIMIT 100"
    )))
    .bind(org)
    .bind(user)
    .fetch_all(exec)
    .await?)
}

/// (C-1047) The organization has an `active` chat integration (of `connector` when given).
pub async fn has_chat_platform<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    connector: Option<&str>,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM channel_integrations WHERE org_id = $1 AND status = 'active' \
         AND ($2::text IS NULL OR connector_id = $2))",
    )
    .bind(org)
    .bind(connector)
    .fetch_one(exec)
    .await?)
}

/// (C-1047) An `active` LLM key with a provider and endpoint exists that is the organization's or the
/// workspace's.
pub async fn has_ai_key<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    workspace: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM secrets WHERE kind = 'llm' AND status = 'active' AND org_id = $1 \
         AND (workspace_id IS NULL OR workspace_id = $2) \
         AND llm_provider IS NOT NULL AND llm_endpoint_id IS NOT NULL)",
    )
    .bind(org)
    .bind(workspace)
    .fetch_one(exec)
    .await?)
}

/// (C-1047) A connector (of `connector` when given) is `active` on the instance.
pub async fn has_active_connector<'e, E: PgExecutor<'e>>(
    exec: E,
    connector: Option<&str>,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM connectors WHERE status = 'active' \
         AND ($1::text IS NULL OR id = $1 OR type_id = $1))",
    )
    .bind(connector)
    .fetch_one(exec)
    .await?)
}

/// Active instance administrators: `user_id`.
pub async fn instance_admins<'e, E: PgExecutor<'e>>(exec: E) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM users WHERE is_instance_admin AND disabled_at IS NULL ORDER BY id",
    )
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
