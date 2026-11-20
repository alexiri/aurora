//! Schedules and their run log (docs/03 §8, docs/10 §Schedules).

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ScheduleRow {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub name: String,
    pub prompt: String,
    /// 5-field cron; `None` for a one-time schedule (P5-SCHED-ONCE, C-1092: exactly one of
    /// `cron` / `run_at`).
    pub cron: Option<String>,
    /// (P5-SCHED-ONCE) The instant of a one-time schedule.
    pub run_at: Option<DateTime<Utc>>,
    /// (P5-SCHED-ONCE) Set when a one-time schedule fired (Done).
    pub completed_at: Option<DateTime<Utc>>,
    pub timezone: String,
    pub gate_command: Option<String>,
    pub gate_description: Option<String>,
    pub enabled: bool,
    pub disabled_reason: Option<String>,
    /// (C-1096) The person whose own pause this is (`disabled_reason = 'user'`); else null.
    pub disabled_by: Option<Uuid>,
    pub disabled_by_name: Option<String>,
    pub consecutive_failures: i32,
    pub creator_user_id: Option<Uuid>,
    pub creator_name: Option<String>,
    /// `creator` | `workspace` (RFE-021): the fire-time authority.
    pub anchor: String,
    pub origin_conversation_id: Option<Uuid>,
    pub run_conversation_id: Option<Uuid>,
    pub next_fire_at: Option<DateTime<Utc>>,
    pub last_fire_at: Option<DateTime<Utc>>,
    pub last_outcome: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str = "s.id, s.agent_id, s.name, s.prompt, s.cron, s.run_at, s.completed_at, \
    s.timezone, s.gate_command, \
    s.gate_description, s.enabled, s.disabled_reason, s.disabled_by, \
    (SELECT d.display_name FROM users d WHERE d.id = s.disabled_by) AS disabled_by_name, \
    s.consecutive_failures, s.creator_user_id, \
    u.display_name AS creator_name, s.anchor, s.origin_conversation_id, s.run_conversation_id, \
    s.next_fire_at, s.last_fire_at, s.last_outcome, s.created_at, s.updated_at";
const FROM: &str = "FROM schedules s LEFT JOIN users u ON u.id = s.creator_user_id";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RunRow {
    pub id: Uuid,
    pub schedule_id: Uuid,
    pub turn_id: Option<Uuid>,
    pub outcome: String,
    pub fired_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

const RUN_COLS: &str = "id, schedule_id, turn_id, outcome, fired_at, created_at";

pub struct NewSchedule<'a> {
    pub agent_id: Uuid,
    pub name: &'a str,
    pub prompt: &'a str,
    pub cron: Option<&'a str>,
    pub run_at: Option<DateTime<Utc>>,
    pub timezone: &'a str,
    pub gate_command: Option<&'a str>,
    pub gate_description: Option<&'a str>,
    pub creator_user_id: Option<Uuid>,
    pub anchor: &'a str,
    pub origin_conversation_id: Option<Uuid>,
    pub next_fire_at: Option<DateTime<Utc>>,
}

pub async fn get<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<ScheduleRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} {FROM} WHERE s.id = $1"
    )))
    .bind(id)
    .fetch_optional(exec)
    .await?)
}

/// (FIX-110) The row, locked for the rest of the transaction: a user-facing edit must start from
/// what a concurrent fire just wrote, not from a stale read.
pub async fn get_for_update(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<ScheduleRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} {FROM} WHERE s.id = $1 FOR UPDATE OF s"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

pub async fn list_for_agent<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
) -> Result<Vec<ScheduleRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} {FROM} WHERE s.agent_id = $1 ORDER BY s.created_at, s.id LIMIT 500"
    )))
    .bind(agent)
    .fetch_all(exec)
    .await?)
}

/// The schedules counting toward the per-agent limit: (P5-SCHED-ONCE, D-421) Done ones don't.
pub async fn count_for_agent<'e, E: PgExecutor<'e>>(exec: E, agent: Uuid) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM schedules WHERE agent_id = $1 AND completed_at IS NULL",
    )
    .bind(agent)
    .fetch_one(exec)
    .await?)
}

pub async fn insert(conn: &mut PgConnection, n: &NewSchedule<'_>) -> Result<ScheduleRow, DbError> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO schedules (id, agent_id, name, prompt, cron, timezone, gate_command,
            gate_description, creator_user_id, anchor, origin_conversation_id, next_fire_at, run_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
    )
    .bind(id)
    .bind(n.agent_id)
    .bind(n.name)
    .bind(n.prompt)
    .bind(n.cron)
    .bind(n.timezone)
    .bind(n.gate_command)
    .bind(n.gate_description)
    .bind(n.creator_user_id)
    .bind(n.anchor)
    .bind(n.origin_conversation_id)
    .bind(n.next_fire_at)
    .bind(n.run_at)
    .execute(&mut *conn)
    .await?;
    get(&mut *conn, id).await?.ok_or(DbError::NotFound)
}

/// Edit of the user-controlled fields. (FIX-110) The fire-time columns (`enabled`,
/// `disabled_reason`, `consecutive_failures`, `next_fire_at`, `fire_lease_until`) are written
/// only with `reschedule` (an enable change or a timing change, which clears the lease); a plain
/// edit never restores a stale `next_fire_at` or clears a live lease (a double fire). Call on a
/// row read with [`get_for_update`].
pub async fn update(
    conn: &mut PgConnection,
    r: &ScheduleRow,
    reschedule: bool,
) -> Result<ScheduleRow, DbError> {
    sqlx::query(
        "UPDATE schedules SET name=$2, prompt=$3, cron=$4, timezone=$5, gate_command=$6,
            gate_description=$7, updated_at=now(),
            enabled = CASE WHEN $12 THEN $8 ELSE enabled END,
            disabled_reason = CASE WHEN $12 THEN $9 ELSE disabled_reason END,
            disabled_by = CASE WHEN $12 THEN $15 ELSE disabled_by END,
            consecutive_failures = CASE WHEN $12 THEN $10 ELSE consecutive_failures END,
            next_fire_at = CASE WHEN $12 THEN $11 ELSE next_fire_at END,
            fire_lease_until = CASE WHEN $12 THEN NULL ELSE fire_lease_until END,
            run_at = $13,
            completed_at = CASE WHEN $12 THEN $14 ELSE completed_at END
         WHERE id=$1",
    )
    .bind(r.id)
    .bind(&r.name)
    .bind(&r.prompt)
    .bind(&r.cron)
    .bind(&r.timezone)
    .bind(&r.gate_command)
    .bind(&r.gate_description)
    .bind(r.enabled)
    .bind(&r.disabled_reason)
    .bind(r.consecutive_failures)
    .bind(r.next_fire_at)
    .bind(reschedule)
    .bind(r.run_at)
    .bind(r.completed_at)
    .bind(r.disabled_by)
    .execute(&mut *conn)
    .await?;
    get(&mut *conn, r.id).await?.ok_or(DbError::NotFound)
}

/// RFE-021: change the fire-time authority (creator and/or anchor). With `re_enable_next` the
/// schedule is re-enabled in the same statement (clears `disabled_reason`, failures and lease,
/// sets `next_fire_at`).
pub async fn set_authority(
    conn: &mut PgConnection,
    id: Uuid,
    creator: Option<Uuid>,
    anchor: &str,
    re_enable_next: Option<DateTime<Utc>>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE schedules SET creator_user_id = $2, anchor = $3, updated_at = now(),
            enabled = enabled OR $4, disabled_reason = CASE WHEN $4 THEN NULL ELSE disabled_reason END,
            disabled_by = CASE WHEN $4 THEN NULL ELSE disabled_by END,
            consecutive_failures = CASE WHEN $4 THEN 0 ELSE consecutive_failures END,
            next_fire_at = CASE WHEN $4 THEN $5 ELSE next_fire_at END,
            fire_lease_until = CASE WHEN $4 THEN NULL ELSE fire_lease_until END
         WHERE id = $1",
    )
    .bind(id)
    .bind(creator)
    .bind(anchor)
    .bind(re_enable_next.is_some())
    .bind(re_enable_next)
    .execute(conn)
    .await?;
    Ok(())
}

/// RFE-021: schedules of a workspace whose creator is `user` (any state), locked for update;
/// optionally narrowed to `ids`. Ordered for deterministic audit rows.
pub async fn lock_by_creator(
    conn: &mut PgConnection,
    workspace: Uuid,
    user: Uuid,
    ids: Option<&[Uuid]>,
) -> Result<Vec<ScheduleRow>, DbError> {
    let locked: Vec<Uuid> = sqlx::query_scalar(
        "SELECT s.id FROM schedules s JOIN agents a ON a.id = s.agent_id
         WHERE a.workspace_id = $1 AND s.creator_user_id = $2
           AND ($3::uuid[] IS NULL OR s.id = ANY($3))
         ORDER BY s.id FOR UPDATE OF s",
    )
    .bind(workspace)
    .bind(user)
    .bind(ids)
    .fetch_all(&mut *conn)
    .await?;
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} {FROM} WHERE s.id = ANY($1) ORDER BY s.id"
    )))
    .bind(&locked)
    .fetch_all(conn)
    .await?)
}

pub async fn delete(conn: &mut PgConnection, id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query("DELETE FROM schedules WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?
        .rows_affected()
        == 1)
}

/// Arm 9: claim due, enabled schedules whose lease is free, stamping the fire lease. Concurrent
/// pollers skip rows locked by each other, so a schedule is claimed once.
pub async fn claim_due(
    conn: &mut PgConnection,
    lease_secs: f64,
    limit: i64,
) -> Result<Vec<ScheduleRow>, DbError> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE schedules SET fire_lease_until = now() + make_interval(secs => $1)
         WHERE id IN (
            SELECT id FROM schedules
            WHERE enabled AND next_fire_at IS NOT NULL AND next_fire_at <= now()
              AND (fire_lease_until IS NULL OR fire_lease_until <= now())
            ORDER BY next_fire_at, id LIMIT $2 FOR UPDATE SKIP LOCKED)
         RETURNING id",
    )
    .bind(lease_secs)
    .bind(limit)
    .fetch_all(&mut *conn)
    .await?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} {FROM} WHERE s.id = ANY($1) ORDER BY s.next_fire_at, s.id"
    )))
    .bind(&ids)
    .fetch_all(conn)
    .await?)
}

/// Record a fire: next occurrence, last fire time and outcome; releases the lease.
pub async fn record_fire(
    conn: &mut PgConnection,
    id: Uuid,
    next: Option<DateTime<Utc>>,
    fired: bool,
    last_outcome: &str,
    run_conversation: Option<Uuid>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE schedules SET next_fire_at = $2, fire_lease_until = NULL,
            last_fire_at = CASE WHEN $3 THEN now() ELSE last_fire_at END, last_outcome = $4,
            run_conversation_id = COALESCE($5, run_conversation_id), updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(next)
    .bind(fired)
    .bind(last_outcome)
    .bind(run_conversation)
    .execute(conn)
    .await?;
    Ok(())
}

/// Run-now bookkeeping: touch the last fire without moving the schedule.
pub async fn record_manual(
    conn: &mut PgConnection,
    id: Uuid,
    run_conversation: Uuid,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE schedules SET last_fire_at = now(), last_outcome = 'ok',
            run_conversation_id = $2 WHERE id = $1",
    )
    .bind(id)
    .bind(run_conversation)
    .execute(conn)
    .await?;
    Ok(())
}

/// (P5-SCHED-ONCE, C-1092) A one-time schedule fired: Done in the fire's transaction. It is
/// never armed again (`next_fire_at` null) until a new `run_at` re-arms it.
pub async fn complete_once(
    conn: &mut PgConnection,
    id: Uuid,
    run_conversation: Option<Uuid>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE schedules SET next_fire_at = NULL, fire_lease_until = NULL, completed_at = now(),
            last_fire_at = now(), last_outcome = 'ok',
            run_conversation_id = COALESCE($2, run_conversation_id), updated_at = now()
         WHERE id = $1 AND run_at IS NOT NULL",
    )
    .bind(id)
    .bind(run_conversation)
    .execute(conn)
    .await?;
    Ok(())
}

/// (P5-SCHED-ONCE, C-1092) A due one-time schedule of a paused agent waits: checked again at
/// `next`; `last_outcome = 'waiting_paused'`. Returns whether it was already waiting (the
/// `waiting_paused` run is written once).
pub async fn wait_paused(
    conn: &mut PgConnection,
    id: Uuid,
    next: DateTime<Utc>,
) -> Result<bool, DbError> {
    let before: Option<Option<String>> = sqlx::query_scalar(
        "UPDATE schedules s SET next_fire_at = $2, fire_lease_until = NULL,
            last_outcome = 'waiting_paused', updated_at = now()
         FROM (SELECT last_outcome FROM schedules WHERE id = $1) old
         WHERE s.id = $1 RETURNING old.last_outcome",
    )
    .bind(id)
    .bind(next)
    .fetch_optional(conn)
    .await?;
    Ok(before.flatten().as_deref() == Some("waiting_paused"))
}

/// (P5-SCHED-ONCE, C-1092) Ids of Done schedules whose `completed_at` is older than
/// `age_secs`, oldest first. Not locked: each is deleted through the ordinary delete path (which
/// takes its locks in the conversation-then-schedule order).
pub async fn done_before<'e, E: PgExecutor<'e>>(
    exec: E,
    age_secs: f64,
    limit: i64,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM schedules
         WHERE completed_at IS NOT NULL AND completed_at < now() - make_interval(secs => $1)
         ORDER BY completed_at, id LIMIT $2",
    )
    .bind(age_secs)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// (P5-SCHED-ONCE, C-1093) The schedule's origin conversation when a one-time run may happen
/// there (SEC-144): it exists, belongs to the schedule's agent, is a direct conversation that is
/// not archived, and its person is the schedule's **current** creator. Turn creation locks the
/// conversation.
pub async fn origin_for_run(
    conn: &mut PgConnection,
    s: &ScheduleRow,
) -> Result<Option<Uuid>, DbError> {
    let (Some(origin), Some(creator)) = (s.origin_conversation_id, s.creator_user_id) else {
        return Ok(None);
    };
    Ok(sqlx::query_scalar(
        "SELECT id FROM conversations
         WHERE id = $1 AND agent_id = $2 AND kind = 'direct' AND direct_user_id = $3
           AND archived_at IS NULL",
    )
    .bind(origin)
    .bind(s.agent_id)
    .bind(creator)
    .fetch_optional(conn)
    .await?)
}

/// (P5-SCHED-ONCE, C-1095) A run of a one-time schedule whose answer is to be pushed.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AnswerPush {
    pub run_id: Uuid,
    pub schedule_name: String,
    pub user_id: Uuid,
    pub conversation_id: Uuid,
    pub agent_handle: String,
    pub workspace_slug: String,
    pub org_slug: String,
}

/// (P5-SCHED-ONCE, C-1095, SEC-144) Claim the `schedule_answer` push of every run that happened
/// in its creator's own chat (the turn's conversation is the schedule's origin, a direct
/// conversation of the schedule's current creator), ended `done`, and is younger than
/// `max_age_secs`. Claimed once (`answer_pushed_at`); recurring runs never run in the origin.
/// Driven by `schedules_completed_idx`: only schedules Done within the window are read.
pub async fn claim_answer_push(
    conn: &mut PgConnection,
    max_age_secs: f64,
    limit: i64,
) -> Result<Vec<AnswerPush>, DbError> {
    Ok(sqlx::query_as(
        "WITH picked AS (
            SELECT r.id FROM schedules s
              JOIN schedule_runs r ON r.schedule_id = s.id
              JOIN turns t ON t.id = r.turn_id
              JOIN conversations c ON c.id = t.conversation_id
            WHERE s.completed_at > now() - make_interval(secs => $1)
              AND r.created_at > now() - make_interval(secs => $1)
              AND r.answer_pushed_at IS NULL AND t.status = 'done'
              AND c.id = s.origin_conversation_id AND c.kind = 'direct'
              AND c.direct_user_id = s.creator_user_id
            ORDER BY r.created_at LIMIT $2 FOR UPDATE OF r SKIP LOCKED)
         UPDATE schedule_runs r SET answer_pushed_at = now()
         FROM picked p, schedules s, conversations c, agents a, workspaces w, organizations o
         WHERE r.id = p.id AND s.id = r.schedule_id AND c.id = s.origin_conversation_id
           AND a.id = s.agent_id AND w.id = a.workspace_id AND o.id = w.org_id
         RETURNING r.id AS run_id, s.name AS schedule_name, c.direct_user_id AS user_id,
            c.id AS conversation_id, a.handle AS agent_handle, w.slug AS workspace_slug,
            o.slug AS org_slug",
    )
    .bind(max_age_secs)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Disable with a reason; `next_fire_at` is cleared and the lease released.
pub async fn disable(
    conn: &mut PgConnection,
    id: Uuid,
    reason: &str,
    last_outcome: Option<&str>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE schedules SET enabled = false, disabled_reason = $2, disabled_by = NULL, next_fire_at = NULL,
            fire_lease_until = NULL, last_outcome = COALESCE($3, last_outcome), updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(reason)
    .bind(last_outcome)
    .execute(conn)
    .await?;
    Ok(())
}

/// Count one failed run; returns the new consecutive-failure count.
pub async fn add_failure(conn: &mut PgConnection, id: Uuid) -> Result<i32, DbError> {
    Ok(sqlx::query_scalar(
        "UPDATE schedules SET consecutive_failures = consecutive_failures + 1,
            last_outcome = 'failed', updated_at = now()
         WHERE id = $1 RETURNING consecutive_failures",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?
    .unwrap_or(0))
}

pub async fn reset_failures(
    conn: &mut PgConnection,
    id: Uuid,
    last_outcome: &str,
) -> Result<(), DbError> {
    sqlx::query("UPDATE schedules SET consecutive_failures = 0, last_outcome = $2 WHERE id = $1")
        .bind(id)
        .bind(last_outcome)
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn insert_run(
    conn: &mut PgConnection,
    schedule: Uuid,
    turn: Option<Uuid>,
    outcome: &str,
) -> Result<RunRow, DbError> {
    insert_run_with_id(conn, Uuid::now_v7(), schedule, turn, outcome).await
}

/// [`insert_run`] with an id chosen by the caller (P5-REFINE-7, C-1074: the run's thread is
/// `schedule:<schedule>:<run>`, created before the run row).
pub async fn insert_run_with_id(
    conn: &mut PgConnection,
    id: Uuid,
    schedule: Uuid,
    turn: Option<Uuid>,
    outcome: &str,
) -> Result<RunRow, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO schedule_runs (id, schedule_id, turn_id, outcome, fired_at)
         VALUES ($1,$2,$3,$4, now()) RETURNING {RUN_COLS}"
    )))
    .bind(id)
    .bind(schedule)
    .bind(turn)
    .bind(outcome)
    .fetch_one(conn)
    .await?)
}

/// Keyset page of a schedule's runs, newest first (`created_at`, `id`; FIX-111).
pub async fn list_runs<'e, E: PgExecutor<'e>>(
    exec: E,
    schedule: Uuid,
    after: Option<(DateTime<Utc>, Uuid)>,
    limit: i64,
) -> Result<Vec<RunRow>, DbError> {
    let (t, id) = after.unzip();
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {RUN_COLS} FROM schedule_runs WHERE schedule_id = $1
           AND ($2::timestamptz IS NULL OR (created_at, id) < ($2, $3))
         ORDER BY created_at DESC, id DESC LIMIT $4"
    )))
    .bind(schedule)
    .bind(t)
    .bind(id)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// (P5-REFINE-7, C-1077) A run with what its turn and thread say.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RunResultRow {
    pub id: Uuid,
    pub schedule_id: Uuid,
    pub turn_id: Option<Uuid>,
    pub outcome: String,
    pub fired_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    /// The run's thread: its turn's conversation, else the thread named after the run.
    pub conversation_id: Option<Uuid>,
    pub turn_status: Option<String>,
    pub turn_error_code: Option<String>,
}

/// The columns of [`RunResultRow`] over `schedule_runs r`.
const RUN_RESULT_SELECT: &str = "SELECT r.id, r.schedule_id, r.turn_id, r.outcome, r.fired_at,
        r.created_at,
        COALESCE(t.conversation_id, (SELECT c.id FROM conversations c
            WHERE c.automation_kind = 'schedule' AND c.automation_id = r.schedule_id
              AND c.external_ref = 'schedule:' || r.schedule_id::text || ':' || r.id::text)) AS conversation_id,
        t.status AS turn_status, t.error_code AS turn_error_code
     FROM schedule_runs r LEFT JOIN turns t ON t.id = r.turn_id";

/// (C-1077) Keyset page of a schedule's runs with their results, newest first.
pub async fn runs_with_results<'e, E: PgExecutor<'e>>(
    exec: E,
    schedule: Uuid,
    after: Option<(DateTime<Utc>, Uuid)>,
    limit: i64,
) -> Result<Vec<RunResultRow>, DbError> {
    let (t, id) = after.unzip();
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{RUN_RESULT_SELECT} WHERE r.schedule_id = $1
           AND ($2::timestamptz IS NULL OR (r.created_at, r.id) < ($2, $3))
         ORDER BY r.created_at DESC, r.id DESC LIMIT $4"
    )))
    .bind(schedule)
    .bind(t)
    .bind(id)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// (C-1077) One run with its result.
pub async fn run_with_result<'e, E: PgExecutor<'e>>(
    exec: E,
    run: Uuid,
) -> Result<Option<RunResultRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{RUN_RESULT_SELECT} WHERE r.id = $1"
    )))
    .bind(run)
    .fetch_optional(exec)
    .await?)
}

/// (C-1077) The newest run of each of `schedules`, with its result (one query per page).
pub async fn latest_runs<'e, E: PgExecutor<'e>>(
    exec: E,
    schedules: &[Uuid],
) -> Result<Vec<RunResultRow>, DbError> {
    if schedules.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{RUN_RESULT_SELECT} WHERE r.id IN (
            SELECT DISTINCT ON (schedule_id) id FROM schedule_runs WHERE schedule_id = ANY($1)
            ORDER BY schedule_id, created_at DESC, id DESC)"
    )))
    .bind(schedules)
    .fetch_all(exec)
    .await?)
}

/// (C-1074) The busy rule: the schedule's latest run that started a turn still has it active
/// (`queued`, `dispatched`, `running`).
pub async fn latest_run_active(conn: &mut PgConnection, schedule: Uuid) -> Result<bool, DbError> {
    let status: Option<String> = sqlx::query_scalar(
        "SELECT t.status FROM schedule_runs r JOIN turns t ON t.id = r.turn_id
         WHERE r.schedule_id = $1 ORDER BY r.created_at DESC, r.id DESC LIMIT 1",
    )
    .bind(schedule)
    .fetch_optional(conn)
    .await?;
    Ok(status.is_some_and(|s| matches!(s.as_str(), "queued" | "dispatched" | "running")))
}

/// Lock the schedule row for the rest of the transaction (P5-REFINE-7: run-now and a fire
/// decide the busy rule one at a time). Returns false when it is gone.
pub async fn lock(conn: &mut PgConnection, id: Uuid) -> Result<bool, DbError> {
    Ok(
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM schedules WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(conn)
            .await?
            .is_some(),
    )
}

/// The run row that carries `turn` (fenced writers lock it).
pub async fn run_by_turn(conn: &mut PgConnection, turn: Uuid) -> Result<Option<RunRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {RUN_COLS} FROM schedule_runs WHERE turn_id = $1 FOR UPDATE"
    )))
    .bind(turn)
    .fetch_optional(conn)
    .await?)
}

/// Fenced outcome change: only from `from`; optionally detaches the turn.
pub async fn set_run_outcome(
    conn: &mut PgConnection,
    run: Uuid,
    from: &str,
    to: &str,
    detach_turn: bool,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE schedule_runs SET outcome = $3, turn_id = CASE WHEN $4 THEN NULL ELSE turn_id END
         WHERE id = $1 AND outcome = $2",
    )
    .bind(run)
    .bind(from)
    .bind(to)
    .bind(detach_turn)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}
