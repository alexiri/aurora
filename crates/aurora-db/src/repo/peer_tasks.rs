//! Peer tasks (docs/07 §Agent links and peer tasks). Transitions are fenced on the expected
//! status so that a task is closed (and reported) exactly once.

use super::{After, split};
use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PeerTaskRow {
    pub id: Uuid,
    pub link_id: Uuid,
    pub from_agent_id: Uuid,
    pub to_agent_id: Uuid,
    pub requester_user_id: Option<Uuid>,
    pub requester_name: Option<String>,
    pub origin_conversation_id: Uuid,
    pub task_conversation_id: Uuid,
    pub status: String,
    pub messages_used: i32,
    pub message_budget: i32,
    /// Wake-ups so far (each one is also counted in `messages_used`).
    pub wakes_used: i32,
    pub report: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const SELECT: &str = "SELECT t.id, t.link_id, t.from_agent_id, t.to_agent_id, \
    t.requester_user_id, u.display_name AS requester_name, t.origin_conversation_id, \
    t.task_conversation_id, t.status, t.messages_used, t.message_budget, t.wakes_used, \
    t.report, t.expires_at, t.created_at, t.updated_at \
    FROM peer_tasks t LEFT JOIN users u ON u.id = t.requester_user_id";

pub struct NewPeerTask {
    pub id: Uuid,
    pub link_id: Uuid,
    pub from_agent_id: Uuid,
    pub to_agent_id: Uuid,
    pub requester_user_id: Option<Uuid>,
    pub origin_conversation_id: Uuid,
    pub task_conversation_id: Uuid,
    /// `queued` | `open`
    pub status: &'static str,
    pub message_budget: i32,
    pub ttl_secs: f64,
}

pub async fn insert(conn: &mut PgConnection, n: &NewPeerTask) -> Result<PeerTaskRow, DbError> {
    sqlx::query(
        "INSERT INTO peer_tasks (id, link_id, from_agent_id, to_agent_id, requester_user_id,
            origin_conversation_id, task_conversation_id, status, messages_used, message_budget,
            expires_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,1,$9, now() + make_interval(secs => $10))",
    )
    .bind(n.id)
    .bind(n.link_id)
    .bind(n.from_agent_id)
    .bind(n.to_agent_id)
    .bind(n.requester_user_id)
    .bind(n.origin_conversation_id)
    .bind(n.task_conversation_id)
    .bind(n.status)
    .bind(n.message_budget)
    .bind(n.ttl_secs)
    .execute(&mut *conn)
    .await?;
    get_hinted(conn, n.id).await?.ok_or(DbError::NotFound)
}

/// (P5-REFINE-9, C-1084) The task after a change, with a timeline hint queued for both agents.
async fn get_hinted(conn: &mut PgConnection, id: Uuid) -> Result<Option<PeerTaskRow>, DbError> {
    let row = get(&mut *conn, id).await?;
    if let Some(r) = &row {
        use super::timeline::{Source, hint};
        hint(&mut *conn, r.from_agent_id, Source::Peer, r.id).await?;
        hint(&mut *conn, r.to_agent_id, Source::Peer, r.id).await?;
    }
    Ok(row)
}

pub async fn get<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<PeerTaskRow>, DbError> {
    Ok(
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE t.id = $1")))
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}

/// Row-locked read (transaction required).
pub async fn get_for_update(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<PeerTaskRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE t.id = $1 FOR UPDATE OF t"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// The task whose `peer` conversation this is.
pub async fn by_task_conversation<'e, E: PgExecutor<'e>>(
    exec: E,
    conversation_id: Uuid,
) -> Result<Option<PeerTaskRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE t.task_conversation_id = $1"
    )))
    .bind(conversation_id)
    .fetch_optional(exec)
    .await?)
}

/// Queued tasks of the ordered pair (FIX-110 cap; call under the link lock).
pub async fn queued_count<'e, E: PgExecutor<'e>>(
    exec: E,
    from: Uuid,
    to: Uuid,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM peer_tasks
           WHERE from_agent_id = $1 AND to_agent_id = $2 AND status = 'queued'",
    )
    .bind(from)
    .bind(to)
    .fetch_one(exec)
    .await?)
}

/// Is there an `open` or `waiting` task for the ordered pair? (FIX-253 security review, high: a
/// `waiting` task occupies the pair's one-open slot exactly like an `open` one — a second task
/// queues behind it instead of opening beside it, which would turn the first task into a zombie
/// when its wake resumed it into the unique index.)
pub async fn open_exists<'e, E: PgExecutor<'e>>(
    exec: E,
    from: Uuid,
    to: Uuid,
) -> Result<bool, DbError> {
    Ok(slot_status(exec, from, to).await?.is_some())
}

/// The status (`open` or `waiting`) of the task that holds the ordered pair's one-open slot, so
/// the queue notice can say whether the earlier task is waiting; `None` when the slot is free.
pub async fn slot_status<'e, E: PgExecutor<'e>>(
    exec: E,
    from: Uuid,
    to: Uuid,
) -> Result<Option<String>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT status FROM peer_tasks
          WHERE from_agent_id = $1 AND to_agent_id = $2 AND status IN ('open', 'waiting')
          ORDER BY created_at LIMIT 1",
    )
    .bind(from)
    .bind(to)
    .fetch_optional(exec)
    .await?)
}

/// The `open` task between two agents in either direction (mid-task messages).
pub async fn open_between<'e, E: PgExecutor<'e>>(
    exec: E,
    x: Uuid,
    y: Uuid,
) -> Result<Vec<PeerTaskRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE t.status = 'open'
           AND ((t.from_agent_id = $1 AND t.to_agent_id = $2)
             OR (t.from_agent_id = $2 AND t.to_agent_id = $1))
         ORDER BY t.created_at, t.id"
    )))
    .bind(x)
    .bind(y)
    .fetch_all(exec)
    .await?)
}

/// Oldest `queued` task of the ordered pair.
pub async fn next_queued(
    conn: &mut PgConnection,
    from: Uuid,
    to: Uuid,
) -> Result<Option<PeerTaskRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE t.from_agent_id = $1 AND t.to_agent_id = $2 AND t.status = 'queued'
         ORDER BY t.created_at, t.id LIMIT 1 FOR UPDATE OF t"
    )))
    .bind(from)
    .bind(to)
    .fetch_optional(conn)
    .await?)
}

/// `queued` -> `open` (fenced).
pub async fn open_queued(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<PeerTaskRow>, DbError> {
    let done = sqlx::query(
        "UPDATE peer_tasks SET status = 'open', updated_at = now()
         WHERE id = $1 AND status = 'queued'",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(None);
    }
    get_hinted(conn, id).await
}

/// Close a `queued`/`open`/`waiting` task (`completed` | `cancelled` | `expired`). Fenced: `None`
/// when the task was already closed, so the caller posts the report only when this returns `Some`.
pub async fn close(
    conn: &mut PgConnection,
    id: Uuid,
    status: &str,
    report: Option<&str>,
) -> Result<Option<PeerTaskRow>, DbError> {
    let done = sqlx::query(
        "UPDATE peer_tasks SET status = $2, report = COALESCE($3, report), updated_at = now()
         WHERE id = $1 AND status IN ('queued', 'open', 'waiting')",
    )
    .bind(id)
    .bind(status)
    .bind(report)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(None);
    }
    get_hinted(conn, id).await
}

/// Count one more message against the budget of an `open` task. `None` when the task is not
/// open or the budget is used up (the caller closes it).
pub async fn count_message(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<PeerTaskRow>, DbError> {
    let done = sqlx::query(
        "UPDATE peer_tasks SET messages_used = messages_used + 1, updated_at = now()
         WHERE id = $1 AND status = 'open' AND messages_used < message_budget",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(None);
    }
    get(conn, id).await
}

/// Tasks of an agent (either side), newest first; `after` is `(created_at, id)`.
pub async fn list_for_agent<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
    status: Option<&str>,
    after: After,
    fetch: i64,
) -> Result<Vec<PeerTaskRow>, DbError> {
    let (t, id) = split(after);
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE (t.from_agent_id = $1 OR t.to_agent_id = $1)
           AND ($2::text IS NULL OR t.status = $2)
           AND ($3::timestamptz IS NULL OR (t.created_at, t.id) < ($3, $4))
         ORDER BY t.created_at DESC, t.id DESC LIMIT $5"
    )))
    .bind(agent)
    .bind(status)
    .bind(t)
    .bind(id)
    .bind(fetch)
    .fetch_all(exec)
    .await?)
}

/// Ids of `queued`/`open`/`waiting` tasks past `expires_at` (oldest first). A `waiting` task
/// still ends by its overall expiry (FIX-253): the requester gets the existing expiry report.
pub async fn overdue_ids<'e, E: PgExecutor<'e>>(exec: E, limit: i64) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM peer_tasks WHERE status IN ('queued', 'open', 'waiting')
           AND expires_at <= now()
         ORDER BY expires_at LIMIT $1",
    )
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// (FIX-253) `waiting` tasks that look as if nothing is left to wake them: no armed watch, no
/// open one-time wake-up and no pending approval in the task conversation. Candidates only: the
/// caller re-checks each under its lock.
pub async fn waiting_without_wake_ids<'e, E: PgExecutor<'e>>(
    exec: E,
    limit: i64,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT t.id FROM peer_tasks t WHERE t.status = 'waiting'
           AND NOT EXISTS (SELECT 1 FROM watches w
                WHERE w.conversation_id = t.task_conversation_id AND w.status = 'armed')
           AND NOT EXISTS (SELECT 1 FROM schedules s
                WHERE s.origin_conversation_id = t.task_conversation_id AND s.enabled
                  AND s.completed_at IS NULL AND s.run_at IS NOT NULL AND s.cron IS NULL)
           AND NOT EXISTS (SELECT 1 FROM approvals a
                WHERE a.conversation_id = t.task_conversation_id AND a.status = 'pending')
         ORDER BY t.updated_at, t.id LIMIT $1",
    )
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// (FIX-253) Whether an approval of this task conversation was answered (or expired) and the
/// requester has not been told yet: its wake is still on the way.
pub async fn has_undelivered_decision<'e, E: PgExecutor<'e>>(
    exec: E,
    task_conversation: Uuid,
    origin_conversation: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM approvals a
          WHERE a.conversation_id = $1 AND a.status IN ('approved', 'denied', 'expired')
            AND NOT EXISTS (SELECT 1 FROM transcript_events e
                 WHERE e.conversation_id = $2 AND e.type = 'notice'
                   AND e.payload->>'code' = 'peer_approval_decided'
                   AND e.payload->>'approval_id' = a.id::text))",
    )
    .bind(task_conversation)
    .bind(origin_conversation)
    .fetch_one(exec)
    .await?)
}

/// (FIX-253, D-490(b)) Move an `open` task to `waiting` (the end of the receiving turn while one
/// of the wake registrations stands). Fenced: `None` when the task is not `open` any more, so
/// the caller posts the waiting notice only when this returns `Some`.
pub async fn set_waiting(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<PeerTaskRow>, DbError> {
    let done = sqlx::query(
        "UPDATE peer_tasks SET status = 'waiting', updated_at = now()
         WHERE id = $1 AND status = 'open'",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(None);
    }
    get_hinted(conn, id).await
}

/// (FIX-253, D-490(b)) The wake (the watch firing, the schedule firing, the approval decision)
/// resumes a `waiting` task of this conversation back to `open` (an `open` one stays open) — the
/// task keeps the grants and policy it had: a wake never extends rights. The wake turn is charged
/// to the message budget and counted. Returns the task id, if a live task was charged. The caller
/// holds the task's row lock and has checked both limits.
pub async fn resume(conn: &mut PgConnection, conversation: Uuid) -> Result<Option<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE peer_tasks SET status = 'open', messages_used = messages_used + 1,
             wakes_used = wakes_used + 1, updated_at = now()
         WHERE task_conversation_id = $1 AND status IN ('open', 'waiting') RETURNING id",
    )
    .bind(conversation)
    .fetch_optional(&mut *conn)
    .await?)
}

/// Lock out concurrent "go waiting" transitions of this requester and receiver until the
/// transaction ends (always in this order), then count the tasks already waiting:
/// `(of the requester, on the receiving agent)`.
pub async fn lock_waiting_counts(
    conn: &mut PgConnection,
    requester: Option<Uuid>,
    receiver: Uuid,
) -> Result<(i64, i64), DbError> {
    for key in [
        format!("peer-waiting:user:{}", requester.unwrap_or_default()),
        format!("peer-waiting:agent:{receiver}"),
    ] {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(key)
            .execute(&mut *conn)
            .await?;
    }
    Ok(sqlx::query_as(
        "SELECT count(*) FILTER (WHERE requester_user_id = $1),
                count(*) FILTER (WHERE to_agent_id = $2)
           FROM peer_tasks
          WHERE status = 'waiting' AND (requester_user_id = $1 OR to_agent_id = $2)",
    )
    .bind(requester)
    .bind(receiver)
    .fetch_one(&mut *conn)
    .await?)
}
