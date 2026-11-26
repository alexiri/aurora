//! Web Push persistence (phase 5, P4-011B-3, D-134, D-266; docs/03 `webpush_vapid_keys`,
//! `push_subscriptions`, `approval_push_claims`). Endpoint URLs and subscription keys live only in
//! the sealed `secret_ref` (never here in the clear); `endpoint_hash` is a one-way hash used to find
//! an endpoint that is already registered, `endpoint_host` is the audit-safe host.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgConnection, PgPool};
use uuid::Uuid;

/// One VAPID key row; `private_ref` is the sealed reference, never the key.
#[derive(Debug, Clone, FromRow)]
pub struct VapidKeyRow {
    pub id: Uuid,
    pub public_key: String,
    pub private_ref: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub retired_at: Option<DateTime<Utc>>,
}

const KEY_COLS: &str = "id, public_key, private_ref, status, created_at, retired_at";

/// A runtime-built statement over this module's own constant column lists and fixed tails (FIX-160:
/// no caller value is ever interpolated; values are always bound).
macro_rules! sqlf {
    ($($t:tt)*) => {
        sqlx::AssertSqlSafe(format!($($t)*))
    };
}

/// Serialises first-start key generation across replicas (C-833): held until the transaction ends.
pub async fn lock_vapid(conn: &mut PgConnection) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('aurora.webpush.vapid'))")
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn active_key(conn: &mut PgConnection) -> Result<Option<VapidKeyRow>, DbError> {
    Ok(sqlx::query_as(sqlf!(
        "SELECT {KEY_COLS} FROM webpush_vapid_keys WHERE status = 'active'"
    ))
    .fetch_optional(conn)
    .await?)
}

pub async fn insert_active_key(
    conn: &mut PgConnection,
    id: Uuid,
    public_key: &str,
    private_ref: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO webpush_vapid_keys (id, public_key, private_ref, status) \
         VALUES ($1, $2, $3, 'active')",
    )
    .bind(id)
    .bind(public_key)
    .bind(private_ref)
    .execute(conn)
    .await?;
    Ok(())
}

/// Retire the active key (rotation); the new one is inserted by the caller in the same transaction.
pub async fn retire_active_key(conn: &mut PgConnection) -> Result<Option<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE webpush_vapid_keys SET status = 'retired', retired_at = now() \
         WHERE status = 'active' RETURNING id",
    )
    .fetch_optional(conn)
    .await?)
}

pub async fn key_by_id(pool: &PgPool, id: Uuid) -> Result<Option<VapidKeyRow>, DbError> {
    Ok(sqlx::query_as(sqlf!(
        "SELECT {KEY_COLS} FROM webpush_vapid_keys WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

/// The key whose public point is `public_key`, active or retired (`push_key_stale` decision).
pub async fn key_by_public(
    pool: &PgPool,
    public_key: &str,
) -> Result<Option<VapidKeyRow>, DbError> {
    Ok(sqlx::query_as(sqlf!(
        "SELECT {KEY_COLS} FROM webpush_vapid_keys WHERE public_key = $1"
    ))
    .bind(public_key)
    .fetch_optional(pool)
    .await?)
}

/// Every key with the number of subscriptions that reference it (`push status`).
pub async fn keys_with_counts(pool: &PgPool) -> Result<Vec<(VapidKeyRow, i64)>, DbError> {
    let rows: Vec<VapidKeyRow> = sqlx::query_as(sqlf!(
        "SELECT {KEY_COLS} FROM webpush_vapid_keys ORDER BY created_at, id"
    ))
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for k in rows {
        let n: i64 =
            sqlx::query_scalar("SELECT count(*) FROM push_subscriptions WHERE vapid_key_id = $1")
                .bind(k.id)
                .fetch_one(pool)
                .await?;
        out.push((k, n));
    }
    Ok(out)
}

/// Retired keys that may be destroyed: unreferenced, or retired for longer than `retire_secs`
/// (their subscriptions are deleted with them by [`destroy_key`]).
pub async fn destroyable_keys(
    pool: &PgPool,
    retire_secs: i64,
) -> Result<Vec<VapidKeyRow>, DbError> {
    Ok(sqlx::query_as(sqlf!(
        "SELECT {KEY_COLS} FROM webpush_vapid_keys k WHERE k.status = 'retired' AND \
         (k.retired_at <= now() - make_interval(secs => $1) \
          OR NOT EXISTS (SELECT 1 FROM push_subscriptions p WHERE p.vapid_key_id = k.id))"
    ))
    .bind(retire_secs as f64)
    .fetch_all(pool)
    .await?)
}

/// Delete a retired key and the subscriptions still on it; returns the subscriptions removed.
pub async fn destroy_key(conn: &mut PgConnection, id: Uuid) -> Result<u64, DbError> {
    let n = sqlx::query("DELETE FROM push_subscriptions WHERE vapid_key_id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    sqlx::query("DELETE FROM webpush_vapid_keys WHERE id = $1 AND status = 'retired'")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(n)
}

/// One subscription row joined with its key's status; `secret_ref` is the sealed
/// `{ endpoint, p256dh, auth }`.
#[derive(Debug, Clone, FromRow)]
pub struct SubRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub session_id: Uuid,
    pub vapid_key_id: Option<Uuid>,
    pub endpoint_host: String,
    pub secret_ref: String,
    pub label: String,
    pub show_details: bool,
    pub paused_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub failure_count: i32,
    pub disabled_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub key_status: Option<String>,
}

const SUB_SELECT: &str = "SELECT p.id, p.user_id, p.session_id, p.vapid_key_id, p.endpoint_host, \
     p.secret_ref, p.label, p.show_details, p.paused_at, p.expires_at, p.failure_count, \
     p.disabled_at, p.last_success_at, p.last_failure_at, p.created_at, k.status AS key_status \
     FROM push_subscriptions p LEFT JOIN webpush_vapid_keys k ON k.id = p.vapid_key_id";

pub async fn list_for_user(pool: &PgPool, user: Uuid) -> Result<Vec<SubRow>, DbError> {
    Ok(sqlx::query_as(sqlf!(
        "{SUB_SELECT} WHERE p.user_id = $1 ORDER BY p.created_at, p.id"
    ))
    .bind(user)
    .fetch_all(pool)
    .await?)
}

pub async fn get_owned(pool: &PgPool, id: Uuid, user: Uuid) -> Result<Option<SubRow>, DbError> {
    Ok(
        sqlx::query_as(sqlf!("{SUB_SELECT} WHERE p.id = $1 AND p.user_id = $2"))
            .bind(id)
            .bind(user)
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn get(pool: &PgPool, id: Uuid) -> Result<Option<SubRow>, DbError> {
    Ok(sqlx::query_as(sqlf!("{SUB_SELECT} WHERE p.id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

pub async fn find_by_hash(
    conn: &mut PgConnection,
    endpoint_hash: &[u8],
) -> Result<Option<SubRow>, DbError> {
    Ok(sqlx::query_as(sqlf!(
        "{SUB_SELECT} WHERE p.endpoint_hash = $1 FOR UPDATE OF p"
    ))
    .bind(endpoint_hash)
    .fetch_optional(conn)
    .await?)
}

pub async fn count_for_user(conn: &mut PgConnection, user: Uuid) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM push_subscriptions WHERE user_id = $1")
            .bind(user)
            .fetch_one(conn)
            .await?,
    )
}

pub struct NewSub<'a> {
    pub id: Uuid,
    pub user_id: Uuid,
    pub session_id: Uuid,
    pub vapid_key_id: Option<Uuid>,
    pub endpoint_hash: &'a [u8],
    pub endpoint_host: &'a str,
    pub secret_ref: &'a str,
    pub label: &'a str,
    pub show_details: bool,
    pub expires_at: Option<DateTime<Utc>>,
}

pub async fn insert(conn: &mut PgConnection, n: &NewSub<'_>) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO push_subscriptions (id, user_id, session_id, vapid_key_id, endpoint_hash, \
         endpoint_host, secret_ref, label, show_details, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(n.id)
    .bind(n.user_id)
    .bind(n.session_id)
    .bind(n.vapid_key_id)
    .bind(n.endpoint_hash)
    .bind(n.endpoint_host)
    .bind(n.secret_ref)
    .bind(n.label)
    .bind(n.show_details)
    .bind(n.expires_at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Re-attach an existing row to `session` (and the key and keys of the new registration); the
/// owner stays. Clears the failure state: a fresh registration is a fresh start.
pub async fn rebind(
    conn: &mut PgConnection,
    id: Uuid,
    b: &Binding<'_>,
    label: Option<&str>,
    show_details: Option<bool>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE push_subscriptions SET session_id = $2, vapid_key_id = $3, secret_ref = $4, \
         expires_at = $5, failure_count = 0, disabled_at = NULL, \
         label = COALESCE($6, label), show_details = COALESCE($7, show_details) \
         WHERE id = $1",
    )
    .bind(id)
    .bind(b.session)
    .bind(b.vapid_key_id)
    .bind(b.secret_ref)
    .bind(b.expires_at)
    .bind(label)
    .bind(show_details)
    .execute(conn)
    .await?;
    Ok(())
}

/// A gesture registration of an endpoint another person holds: the row moves to `user` (D-266(c)).
/// Pause and show-details reset to the new person's choices.
pub async fn move_to(
    conn: &mut PgConnection,
    id: Uuid,
    user: Uuid,
    b: &Binding<'_>,
    label: &str,
    show_details: bool,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE push_subscriptions SET user_id = $2, session_id = $3, vapid_key_id = $4, \
         secret_ref = $5, expires_at = $6, label = $7, show_details = $8, paused_at = NULL, \
         failure_count = 0, disabled_at = NULL, last_success_at = NULL, last_failure_at = NULL \
         WHERE id = $1",
    )
    .bind(id)
    .bind(user)
    .bind(b.session)
    .bind(b.vapid_key_id)
    .bind(b.secret_ref)
    .bind(b.expires_at)
    .bind(label)
    .bind(show_details)
    .execute(conn)
    .await?;
    Ok(())
}

/// What a (re-)registration attaches a row to: the registering session, the key in force, the
/// freshly sealed material and the browser's expiry.
pub struct Binding<'a> {
    pub session: Uuid,
    pub vapid_key_id: Option<Uuid>,
    pub secret_ref: &'a str,
    pub expires_at: Option<DateTime<Utc>>,
}

pub async fn update(
    pool: &PgPool,
    id: Uuid,
    user: Uuid,
    label: Option<&str>,
    show_details: Option<bool>,
    paused: Option<bool>,
) -> Result<bool, DbError> {
    let r = sqlx::query(
        "UPDATE push_subscriptions SET label = COALESCE($3, label), \
         show_details = COALESCE($4, show_details), \
         paused_at = CASE WHEN $5::boolean IS NULL THEN paused_at \
                          WHEN $5 THEN COALESCE(paused_at, now()) ELSE NULL END \
         WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user)
    .bind(label)
    .bind(show_details)
    .bind(paused)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// Delete one of the user's own rows; returns its host (for the audit row) when it existed.
pub async fn delete_owned(
    pool: &PgPool,
    id: Uuid,
    user: Uuid,
) -> Result<Option<(String, String)>, DbError> {
    Ok(sqlx::query_as(
        "DELETE FROM push_subscriptions WHERE id = $1 AND user_id = $2 \
         RETURNING endpoint_host, label",
    )
    .bind(id)
    .bind(user)
    .fetch_optional(pool)
    .await?)
}

/// A push service said the subscription is gone (404/410): delete it.
pub async fn delete_by_id(pool: &PgPool, id: Uuid) -> Result<(), DbError> {
    sqlx::query("DELETE FROM push_subscriptions WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Session revocation (D-266(c)): delete the subscriptions of one session, in the revoking
/// transaction. Returns the number removed (audit `push_subscriptions_removed`).
pub async fn delete_for_session(conn: &mut PgConnection, session: Uuid) -> Result<u64, DbError> {
    Ok(
        sqlx::query("DELETE FROM push_subscriptions WHERE session_id = $1")
            .bind(session)
            .execute(conn)
            .await?
            .rows_affected(),
    )
}

/// Revocation of all of a user's sessions: delete the subscriptions of every revoked session of
/// the user (the rows of the sessions the same transaction just revoked included).
pub async fn delete_for_revoked_sessions_of_user(
    conn: &mut PgConnection,
    user: Uuid,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM push_subscriptions p USING sessions s \
         WHERE p.session_id = s.id AND s.user_id = $1 AND s.revoked_at IS NOT NULL",
    )
    .bind(user)
    .execute(conn)
    .await?
    .rows_affected())
}

/// Daily pruning (D-266(c)): subscriptions whose session is revoked or past its absolute expiry
/// are deleted (the FK cascade covers a pruned session row).
pub async fn prune_dead_sessions(pool: &PgPool) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM push_subscriptions p USING sessions s \
         WHERE p.session_id = s.id AND (s.revoked_at IS NOT NULL OR s.expires_at <= now())",
    )
    .execute(pool)
    .await?
    .rows_affected())
}

/// Count by derived state and by host for `push status` and the gauge. States follow
/// `domains::push::state_of`: disabled, expired, paused, stale_key, active.
pub async fn counts_by_state(pool: &PgPool) -> Result<Vec<(String, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT CASE WHEN p.disabled_at IS NOT NULL THEN 'disabled' \
                     WHEN p.expires_at IS NOT NULL AND p.expires_at <= now() THEN 'expired' \
                     WHEN p.paused_at IS NOT NULL THEN 'paused' \
                     WHEN k.status = 'retired' THEN 'stale_key' ELSE 'active' END AS state, \
                count(*) FROM push_subscriptions p \
         LEFT JOIN webpush_vapid_keys k ON k.id = p.vapid_key_id GROUP BY 1",
    )
    .fetch_all(pool)
    .await?)
}

pub async fn counts_by_host(pool: &PgPool) -> Result<Vec<(String, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT endpoint_host, count(*) FROM push_subscriptions GROUP BY 1 ORDER BY 1",
    )
    .fetch_all(pool)
    .await?)
}

/// Record a send outcome (C-834): success clears the failure streak; a failure bumps it and
/// disables the row at `disable_after` consecutive failures.
pub async fn record_outcome(
    pool: &PgPool,
    id: Uuid,
    success: bool,
    disable_after: i32,
) -> Result<(), DbError> {
    if success {
        sqlx::query(
            "UPDATE push_subscriptions SET failure_count = 0, last_success_at = now() WHERE id = $1",
        )
        .bind(id)
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            "UPDATE push_subscriptions SET failure_count = failure_count + 1, \
             last_failure_at = now(), \
             disabled_at = CASE WHEN failure_count + 1 >= $2 THEN COALESCE(disabled_at, now()) \
                                ELSE disabled_at END WHERE id = $1",
        )
        .bind(id)
        .bind(disable_after)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Disable a row at once (`rejected_host`: the endpoint's host left the allow-list).
pub async fn disable(pool: &PgPool, id: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE push_subscriptions SET disabled_at = COALESCE(disabled_at, now()), \
         last_failure_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// A pending approval a push may be about. Free text is limited to the agent and workspace names
/// (sent only to a device with `show_details`) and the slugs of the dashboard path; never a title,
/// summary, rule, host, path or body.
#[derive(Debug, Clone, FromRow)]
pub struct Waiting {
    pub id: Uuid,
    /// `gateway` | `action`
    pub kind: String,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub batch_id: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub parked: bool,
    pub agent_name: String,
    pub workspace_name: String,
    pub workspace_slug: String,
    pub org_slug: String,
}

const WAITING_GATEWAY: &str = "SELECT a.id, 'gateway'::text AS kind, a.org_id, a.workspace_id, \
        a.agent_id, a.conversation_id, a.batch_id, a.deadline AS expires_at, a.parked, \
        a.created_at, COALESCE(ag.name, 'An agent') AS agent_name, w.name AS workspace_name, \
        w.slug AS workspace_slug, o.slug AS org_slug \
     FROM approvals a JOIN workspaces w ON w.id = a.workspace_id \
     JOIN organizations o ON o.id = w.org_id LEFT JOIN agents ag ON ag.id = a.agent_id";

const WAITING_ACTION: &str = "SELECT a.id, 'action'::text AS kind, w.org_id, a.workspace_id, \
        a.agent_id, a.conversation_id, NULL::text AS batch_id, a.expires_at, false AS parked, \
        a.created_at, COALESCE(ag.name, 'An agent') AS agent_name, w.name AS workspace_name, \
        w.slug AS workspace_slug, o.slug AS org_slug \
     FROM action_approvals a JOIN workspaces w ON w.id = a.workspace_id \
     JOIN organizations o ON o.id = w.org_id LEFT JOIN agents ag ON ag.id = a.agent_id";

/// No decider of the approval's workspace holds a watcher row seen within `$1` seconds (a live
/// Aurora tab: the dashboard already shows the approval).
fn no_live_watcher() -> String {
    format!(
        "NOT EXISTS (SELECT 1 FROM approval_watchers wt WHERE wt.workspace_id = a.workspace_id \
           AND wt.last_seen_at > now() - make_interval(secs => $1) AND {})",
        super::approval_notify::may_decide("wt.user_id")
    )
}

/// Approvals still `pending` and decidable that are at least `delay_secs` old and at most
/// `catchup_secs` old (the delay plus the catch-up window of C-834), with no live watcher. Nothing
/// is returned while no subscription exists at all (one indexed probe per sweep).
pub async fn waiting(
    pool: &PgPool,
    delay_secs: f64,
    catchup_secs: f64,
    watcher_ttl_secs: f64,
    limit: i64,
) -> Result<Vec<Waiting>, DbError> {
    let nw = no_live_watcher();
    Ok(sqlx::query_as(sqlf!(
        "SELECT id, kind, org_id, workspace_id, agent_id, conversation_id, batch_id, expires_at, \
                parked, agent_name, workspace_name, workspace_slug, org_slug FROM ( \
           {WAITING_GATEWAY} WHERE a.status = 'pending' AND a.deadline > now() AND {nw} \
             AND a.created_at <= now() - make_interval(secs => $2) \
             AND a.created_at > now() - make_interval(secs => $3) \
           UNION ALL \
           {WAITING_ACTION} WHERE a.status = 'pending' AND a.expires_at > now() AND {nw} \
             AND a.created_at <= now() - make_interval(secs => $2) \
             AND a.created_at > now() - make_interval(secs => $3)) u \
         WHERE EXISTS (SELECT 1 FROM push_subscriptions) ORDER BY created_at LIMIT $4"
    ))
    .bind(watcher_ttl_secs)
    .bind(delay_secs)
    .bind(catchup_secs)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// (FIX-252) A gateway approval raised in a peer task conversation, still `pending` after
/// `delay_secs`, with the person who asked for the task: their devices are pushed about the
/// wait once the delay passes (the shared `(approval, user, stage)` claim with the deciders'
/// pushes means a requester who is also a decider is served once). Only users who have a
/// subscription at all.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RequesterWaiting {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub expires_at: DateTime<Utc>,
    pub agent_name: String,
    pub workspace_name: String,
    pub workspace_slug: String,
    pub org_slug: String,
    pub requester_user_id: Uuid,
}

pub async fn waiting_for_requesters(
    pool: &PgPool,
    delay_secs: f64,
    limit: i64,
) -> Result<Vec<RequesterWaiting>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(
        "SELECT a.id, a.workspace_id, a.deadline AS expires_at, \
                COALESCE(ag.name, 'An agent') AS agent_name, w.name AS workspace_name, \
                w.slug AS workspace_slug, o.slug AS org_slug, pt.requester_user_id \
         FROM approvals a \
         JOIN peer_tasks pt ON pt.task_conversation_id = a.conversation_id \
         JOIN workspaces w ON w.id = a.workspace_id \
         JOIN organizations o ON o.id = w.org_id \
         LEFT JOIN agents ag ON ag.id = a.agent_id \
         WHERE a.status = 'pending' AND a.deadline > now() \
           AND a.created_at <= now() - make_interval(secs => $1) \
           AND EXISTS (SELECT 1 FROM push_subscriptions ps WHERE ps.user_id = pt.requester_user_id) \
           AND NOT EXISTS (SELECT 1 FROM approval_push_claims c \
                           WHERE c.approval_id = a.id AND c.user_id = pt.requester_user_id \
                             AND c.stage = 'initial') \
         ORDER BY a.created_at LIMIT $2",
    ))
    .bind(delay_secs)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Parked gateway approvals past half of their waiting time, still `pending`, with no live
/// watcher (the reminder stage, C-828/C-834).
pub async fn reminders(
    pool: &PgPool,
    watcher_ttl_secs: f64,
    limit: i64,
) -> Result<Vec<Waiting>, DbError> {
    let nw = no_live_watcher();
    Ok(sqlx::query_as(sqlf!(
        "SELECT id, kind, org_id, workspace_id, agent_id, conversation_id, batch_id, expires_at, \
                parked, agent_name, workspace_name, workspace_slug, org_slug FROM ( \
           {WAITING_GATEWAY} WHERE a.status = 'pending' AND a.parked AND a.deadline > now() \
             AND now() >= a.created_at + (a.deadline - a.created_at) / 2 AND {nw}) u \
         WHERE EXISTS (SELECT 1 FROM push_subscriptions) ORDER BY created_at LIMIT $2"
    ))
    .bind(watcher_ttl_secs)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Is the approval still `pending` and unexpired? Checked immediately before claiming (C-834).
pub async fn still_pending(pool: &PgPool, id: Uuid, kind: &str) -> Result<bool, DbError> {
    let table = if kind == "action" {
        "SELECT 1 FROM action_approvals WHERE id = $1 AND status = 'pending' AND expires_at > now()"
    } else {
        "SELECT 1 FROM approvals WHERE id = $1 AND status = 'pending' AND deadline > now()"
    };
    Ok(sqlx::query_scalar::<_, i32>(table)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .is_some())
}

/// Has `(approval, user, stage)` been claimed already (by any replica)?
pub async fn claimed(
    pool: &PgPool,
    approval: Uuid,
    user: Uuid,
    stage: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM approval_push_claims \
         WHERE approval_id = $1 AND user_id = $2 AND stage = $3)",
    )
    .bind(approval)
    .bind(user)
    .bind(stage)
    .fetch_one(pool)
    .await?)
}

/// Claim `(approval, user, stage)`; `true` only for the replica whose insert won. `delivery`:
/// `push` | `batch` | `digest`.
pub async fn claim(
    pool: &PgPool,
    approval: Uuid,
    user: Uuid,
    kind: &str,
    stage: &str,
    delivery: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "INSERT INTO approval_push_claims (approval_id, user_id, stage, kind, delivery) \
         VALUES ($1, $2, $3, $4, $5) ON CONFLICT (approval_id, user_id, stage) DO NOTHING",
    )
    .bind(approval)
    .bind(user)
    .bind(stage)
    .bind(kind)
    .bind(delivery)
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}

/// Has `user` already been served (pushed or folded) for another approval of this batch?
pub async fn batch_served(
    pool: &PgPool,
    user: Uuid,
    agent: Uuid,
    batch_id: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM approval_push_claims n \
         JOIN approvals a ON a.id = n.approval_id \
         WHERE n.user_id = $1 AND a.agent_id = $2 AND a.batch_id = $3 AND n.stage = 'initial')",
    )
    .bind(user)
    .bind(agent)
    .bind(batch_id)
    .fetch_one(pool)
    .await?)
}

/// Other pending approvals of the batch (same agent): folded into one push.
pub async fn batch_siblings(
    pool: &PgPool,
    agent: Uuid,
    batch_id: &str,
    except: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT a.id FROM approvals a WHERE a.agent_id = $1 AND a.batch_id = $2 AND a.id <> $3 \
           AND a.status = 'pending' AND a.deadline > now() ORDER BY a.created_at, a.id",
    )
    .bind(agent)
    .bind(batch_id)
    .bind(except)
    .fetch_all(pool)
    .await?)
}

/// Per-user counters for the hourly cap: individual pushes claimed in the last hour, and whether a
/// digest was claimed in it.
pub async fn hourly(pool: &PgPool, user: Uuid) -> Result<(i64, bool), DbError> {
    Ok(sqlx::query_as(
        "SELECT count(*) FILTER (WHERE delivery = 'push'), \
                COALESCE(bool_or(delivery = 'digest'), false) \
         FROM approval_push_claims WHERE user_id = $1 AND sent_at > now() - interval '1 hour'",
    )
    .bind(user)
    .fetch_one(pool)
    .await?)
}

/// Delete claims older than `days` (the daily retention pruning, with the e-mail claims).
pub async fn prune_claims(pool: &PgPool, days: i32) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM approval_push_claims WHERE sent_at < now() - make_interval(days => $1)",
    )
    .bind(days)
    .execute(pool)
    .await?
    .rows_affected())
}
