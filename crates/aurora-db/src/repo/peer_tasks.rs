//! Peer tasks (docs/07 §Agent links and peer tasks). Transitions are fenced on the expected
//! status so that a task is closed (and reported) exactly once.

use super::{After, split};
use crate::DbError;
use aurora_core::limits::PEER_CHAIN_DEPTH_MAX;
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

/// (FIX-257) The conversations that closing task `id` may write into and that are task
/// conversations themselves: its origin, and the origins of its pair's queued tasks (the
/// promotion after the close may end one of those). (FIX-288) And every task conversation above
/// those: a report wakes the delegating task, whose own close writes one level higher. Newest
/// first, so a task conversation comes before the one it was opened from.
pub async fn task_origins<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "WITH RECURSIVE {up} SELECT DISTINCT origin FROM up
          WHERE EXISTS (SELECT 1 FROM peer_tasks p WHERE p.task_conversation_id = up.origin)
          ORDER BY 1 DESC",
        up = UP_FROM_TASK
    )))
    .bind(id)
    .bind(PEER_CHAIN_DEPTH_MAX)
    .fetch_all(exec)
    .await?)
}

/// (FIX-288) The recursive part of [`task_origins`] and [`lock_chain_links`]: `$1` the task
/// (with its pair's queued tasks), `$2` the depth bound. One row per task on the way up:
/// its origin and its link.
const UP_FROM_TASK: &str = "up (origin, link_id, depth) AS (
        SELECT t.origin_conversation_id, t.link_id, 1 FROM peer_tasks c
          JOIN peer_tasks t ON t.id = c.id
            OR (t.from_agent_id = c.from_agent_id AND t.to_agent_id = c.to_agent_id
                AND t.status = 'queued')
         WHERE c.id = $1
        UNION
        SELECT p.origin_conversation_id, p.link_id, up.depth + 1
          FROM up JOIN peer_tasks p ON p.task_conversation_id = up.origin
         WHERE up.depth < $2
    )";

/// (FIX-288) Row-locks the link of task `id` and of every task above it (and above its pair's
/// queued tasks), in id order, so that two closes whose chains share links take them alike.
pub async fn lock_chain_links(conn: &mut PgConnection, id: Uuid) -> Result<(), DbError> {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "WITH RECURSIVE {up} SELECT l.id FROM agent_links l
          WHERE l.id IN (SELECT link_id FROM up) ORDER BY l.id FOR UPDATE OF l",
        up = UP_FROM_TASK
    )))
    .bind(id)
    .bind(PEER_CHAIN_DEPTH_MAX)
    .execute(conn)
    .await?;
    Ok(())
}

/// (FIX-257) The same for everything that unlinking a pair writes into: the origins of its live
/// tasks, and the conversations its pending `message_agent` approvals were asked in; (FIX-288)
/// with every task conversation above them.
pub async fn link_origins<'e, E: PgExecutor<'e>>(
    exec: E,
    link: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "WITH RECURSIVE up (id, depth) AS (
            SELECT o.id, 1 FROM (
                SELECT origin_conversation_id AS id FROM peer_tasks
                 WHERE link_id = $1 AND status IN ('queued', 'open', 'waiting')
                UNION
                SELECT conversation_id FROM action_approvals
                 WHERE action_key = 'message_agent' AND status = 'pending'
                   AND payload->>'link_id' = $1::text) o
            UNION
            SELECT p.origin_conversation_id, up.depth + 1
              FROM up JOIN peer_tasks p ON p.task_conversation_id = up.id
             WHERE up.depth < $2
         )
         SELECT DISTINCT id FROM up
          WHERE EXISTS (SELECT 1 FROM peer_tasks p WHERE p.task_conversation_id = up.id)
          ORDER BY 1 DESC",
    )
    .bind(link)
    .bind(PEER_CHAIN_DEPTH_MAX)
    .fetch_all(exec)
    .await?)
}

/// (FIX-288, D-509) The conversation an approval of task `id` is reported to
/// (`peer_task_card_home`, the one statement of the walk). `None`: no such task.
pub async fn card_home<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<Uuid>, DbError> {
    Ok(sqlx::query_scalar("SELECT peer_task_card_home($1, $2)")
        .bind(id)
        .bind(PEER_CHAIN_DEPTH_MAX)
        .fetch_one(exec)
        .await?)
}

/// (FIX-288) The conversations the delegating agents above task `id` work in, from its origin up
/// to `home` ([`card_home`]).
pub async fn delegating_conversations<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    home: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    let all: Vec<Uuid> = sqlx::query_scalar(
        "WITH RECURSIVE up (origin, depth) AS (
            SELECT origin_conversation_id, 1 FROM peer_tasks WHERE id = $1
            UNION ALL
            SELECT p.origin_conversation_id, up.depth + 1
              FROM up JOIN peer_tasks p ON p.task_conversation_id = up.origin
             WHERE up.depth < $2
         )
         SELECT origin FROM up ORDER BY depth",
    )
    .bind(id)
    .bind(PEER_CHAIN_DEPTH_MAX)
    .fetch_all(exec)
    .await?;
    let end = all.iter().position(|c| *c == home).map_or(0, |i| i + 1);
    Ok(all.into_iter().take(end).collect())
}

/// (FIX-288) How many tasks lie above and including the one whose conversation this is: 0 for a
/// conversation that is no task conversation. A task opened from here is one deeper.
pub async fn chain_depth<'e, E: PgExecutor<'e>>(
    exec: E,
    conversation: Uuid,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "WITH RECURSIVE up (origin, depth) AS (
            SELECT origin_conversation_id, 1 FROM peer_tasks WHERE task_conversation_id = $1
            UNION ALL
            SELECT p.origin_conversation_id, up.depth + 1
              FROM up JOIN peer_tasks p ON p.task_conversation_id = up.origin
             WHERE up.depth < $2
         )
         SELECT count(*) FROM up",
    )
    .bind(conversation)
    .bind(PEER_CHAIN_DEPTH_MAX)
    .fetch_one(exec)
    .await?)
}

/// (FIX-288, D-509) Whether a task opened from `conversation` is still live (`queued`, `open` or
/// `waiting`): its report is a wake still to come for the task working in that conversation.
pub async fn has_live_onward<'e, E: PgExecutor<'e>>(
    exec: E,
    conversation: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM peer_tasks
          WHERE origin_conversation_id = $1 AND status IN ('queued', 'open', 'waiting'))",
    )
    .bind(conversation)
    .fetch_one(exec)
    .await?)
}

/// (FIX-288) A wake-up held for a delegating task whose agent is frozen.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct HeldWake {
    pub about_task: Uuid,
    pub text: String,
}

/// (FIX-288) Holds a wake-up of `task` (once per `source`).
pub async fn hold_wake<'e, E: PgExecutor<'e>>(
    exec: E,
    task: Uuid,
    source: &str,
    about_task: Uuid,
    text: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO peer_held_wakes (task_id, source, about_task, text) VALUES ($1,$2,$3,$4)
         ON CONFLICT (task_id, source) DO NOTHING",
    )
    .bind(task)
    .bind(source)
    .bind(about_task)
    .bind(text)
    .execute(exec)
    .await?;
    Ok(())
}

pub async fn has_held_wake<'e, E: PgExecutor<'e>>(exec: E, task: Uuid) -> Result<bool, DbError> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM peer_held_wakes WHERE task_id = $1)")
            .bind(task)
            .fetch_one(exec)
            .await?,
    )
}

/// (FIX-288) Takes (deletes) the held wake-ups of `task`, oldest first.
pub async fn take_held_wakes(
    conn: &mut PgConnection,
    task: Uuid,
) -> Result<Vec<HeldWake>, DbError> {
    let mut rows: Vec<(Uuid, String, DateTime<Utc>)> = sqlx::query_as(
        "DELETE FROM peer_held_wakes WHERE task_id = $1 RETURNING about_task, text, created_at",
    )
    .bind(task)
    .fetch_all(conn)
    .await?;
    rows.sort_by_key(|r| r.2);
    Ok(rows
        .into_iter()
        .map(|(about_task, text, _)| HeldWake { about_task, text })
        .collect())
}

/// (FIX-288) Tasks with a held wake-up that can be dealt with now: the task has ended (the rows
/// are dropped) or its agent, workspace and organization are no longer frozen. Frozen ones are
/// left out so they fill no batch.
pub async fn tasks_with_deliverable_held_wakes<'e, E: PgExecutor<'e>>(
    exec: E,
    limit: i64,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT t.id FROM peer_tasks t
           JOIN agents a ON a.id = t.to_agent_id
           JOIN workspaces w ON w.id = a.workspace_id
           JOIN organizations o ON o.id = w.org_id
          WHERE EXISTS (SELECT 1 FROM peer_held_wakes h WHERE h.task_id = t.id)
            AND (t.status NOT IN ('open', 'waiting')
                 OR (a.frozen_at IS NULL AND w.frozen_at IS NULL AND o.frozen_at IS NULL))
          ORDER BY t.updated_at, t.id LIMIT $1",
    )
    .bind(limit)
    .fetch_all(exec)
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

/// (FIX-288) SQL: the requester has not been told the outcome of approval `a`, raised in task
/// `t`'s conversation. The notice is at the task's [`card_home`]; `$2` is the depth bound.
const UNTOLD: &str = "NOT EXISTS (SELECT 1 FROM transcript_events e
     WHERE e.conversation_id = peer_task_card_home(t.id, $2) AND e.type = 'notice'
       AND e.payload->>'code' = 'peer_approval_decided'
       AND e.payload->>'approval_id' = a.id::text)";

/// (FIX-253) `waiting` tasks that look as if nothing is left to wake them: no armed watch, no
/// open one-time wake-up, no pending approval in the task conversation and no decision still to
/// be delivered (those, held by a freeze for as long as it lasts, are [`held_decision_ids`]'s;
/// listed here they would fill the batch and starve the rest), and (FIX-288) no live onward task
/// and no held wake-up. Candidates only: the caller re-checks each under its lock.
pub async fn waiting_without_wake_ids<'e, E: PgExecutor<'e>>(
    exec: E,
    limit: i64,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT t.id FROM peer_tasks t WHERE t.status = 'waiting'
           AND NOT EXISTS (SELECT 1 FROM peer_tasks c
                WHERE c.origin_conversation_id = t.task_conversation_id
                  AND c.status IN ('queued', 'open', 'waiting'))
           AND NOT EXISTS (SELECT 1 FROM peer_held_wakes h WHERE h.task_id = t.id)
           AND NOT EXISTS (SELECT 1 FROM watches w
                WHERE w.conversation_id = t.task_conversation_id AND w.status = 'armed')
           AND NOT EXISTS (SELECT 1 FROM schedules s
                WHERE s.origin_conversation_id = t.task_conversation_id AND s.enabled
                  AND s.completed_at IS NULL AND s.run_at IS NOT NULL AND s.cron IS NULL
                  AND s.agent_id = t.to_agent_id AND s.ends_at IS NOT NULL)
           AND NOT EXISTS (SELECT 1 FROM approvals a
                WHERE a.conversation_id = t.task_conversation_id AND a.status = 'pending')
           AND NOT EXISTS (SELECT 1 FROM approvals a
                WHERE a.conversation_id = t.task_conversation_id
                  AND a.status IN ('approved', 'denied', 'expired')
                  AND {UNTOLD})
         ORDER BY t.updated_at, t.id LIMIT $1"
    )))
    .bind(limit)
    .bind(PEER_CHAIN_DEPTH_MAX)
    .fetch_all(exec)
    .await?)
}

/// (FIX-253) Whether an approval of task `task`'s conversation was answered (or expired) and the
/// requester has not been told yet: its wake is still on the way.
pub async fn has_undelivered_decision<'e, E: PgExecutor<'e>>(
    exec: E,
    task: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT EXISTS (SELECT 1 FROM peer_tasks t
           JOIN approvals a ON a.conversation_id = t.task_conversation_id
          WHERE t.id = $1 AND a.status IN ('approved', 'denied', 'expired') AND {UNTOLD})"
    )))
    .bind(task)
    .bind(PEER_CHAIN_DEPTH_MAX)
    .fetch_one(exec)
    .await?)
}

/// (FIX-253) Answered (or expired) approvals of `waiting` tasks whose requester has not been
/// told: their delivery was held (a frozen receiver).
pub async fn held_decision_ids<'e, E: PgExecutor<'e>>(
    exec: E,
    limit: i64,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT a.id FROM peer_tasks t
           JOIN approvals a ON a.conversation_id = t.task_conversation_id
          WHERE t.status = 'waiting' AND a.status IN ('approved', 'denied', 'expired')
            AND {UNTOLD}
          ORDER BY t.updated_at, a.id LIMIT $1"
    )))
    .bind(limit)
    .bind(PEER_CHAIN_DEPTH_MAX)
    .fetch_all(exec)
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

/// Count the tasks already waiting: `(of the requester, on the receiving agent)`. It first
/// *tries* the transaction locks of this requester and receiver, which keep concurrent "go
/// waiting" transitions from counting past each other, and counts without a lock another one
/// holds: it **never waits**. The caller holds its conversation and goes on to lock the origin
/// conversation, which for chained tasks is another turn end's conversation, so waiting here would
/// deadlock. The price: transitions that overlap may pass a bound by at most one task for each
/// of the others in flight.
pub async fn count_waiting(
    conn: &mut PgConnection,
    requester: Option<Uuid>,
    receiver: Uuid,
) -> Result<(i64, i64), DbError> {
    for key in [
        format!("peer-waiting:user:{}", requester.unwrap_or_default()),
        format!("peer-waiting:agent:{receiver}"),
    ] {
        sqlx::query("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
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
