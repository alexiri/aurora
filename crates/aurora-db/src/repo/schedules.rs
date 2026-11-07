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
    pub cron: String,
    pub timezone: String,
    pub gate_command: Option<String>,
    pub gate_description: Option<String>,
    pub enabled: bool,
    pub disabled_reason: Option<String>,
    pub consecutive_failures: i32,
    pub creator_user_id: Option<Uuid>,
    pub creator_name: Option<String>,
    /// `creator` | `workspace` (RFE-021): the fire-time authority.
    pub anchor: String,
    pub origin_conversation_id: Option<Uuid>,
    pub run_conversation_id: Option<Uuid>,
    pub next_fire_at: Option<DateTime<Utc>>,
    pub last_fire_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str = "s.id, s.agent_id, s.name, s.prompt, s.cron, s.timezone, s.gate_command, \
    s.gate_description, s.enabled, s.disabled_reason, s.consecutive_failures, s.creator_user_id, \
    u.display_name AS creator_name, s.anchor, s.origin_conversation_id, s.run_conversation_id, \
    s.next_fire_at, s.last_fire_at, s.created_at, s.updated_at";
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
    pub cron: &'a str,
    pub timezone: &'a str,
    pub gate_command: Option<&'a str>,
    pub gate_description: Option<&'a str>,
    pub creator_user_id: Option<Uuid>,
    pub anchor: &'a str,
    pub origin_conversation_id: Option<Uuid>,
    pub next_fire_at: Option<DateTime<Utc>>,
}

pub async fn get<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<ScheduleRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("SELECT {COLS} {FROM} WHERE s.id = $1"))
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}

pub async fn list_for_agent<'e, E: PgExecutor<'e>>(
    exec: E,
    agent: Uuid,
) -> Result<Vec<ScheduleRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} {FROM} WHERE s.agent_id = $1 ORDER BY s.created_at, s.id LIMIT 500"
    ))
    .bind(agent)
    .fetch_all(exec)
    .await?)
}

pub async fn count_for_agent<'e, E: PgExecutor<'e>>(exec: E, agent: Uuid) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM schedules WHERE agent_id = $1")
            .bind(agent)
            .fetch_one(exec)
            .await?,
    )
}

pub async fn insert(conn: &mut PgConnection, n: &NewSchedule<'_>) -> Result<ScheduleRow, DbError> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO schedules (id, agent_id, name, prompt, cron, timezone, gate_command,
            gate_description, creator_user_id, anchor, origin_conversation_id, next_fire_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
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
    .execute(&mut *conn)
    .await?;
    get(&mut *conn, id).await?.ok_or(DbError::NotFound)
}

/// Full-row edit of the user-controlled fields; clears the lease and failure state when the
/// schedule is (re-)enabled.
pub async fn update(conn: &mut PgConnection, r: &ScheduleRow) -> Result<ScheduleRow, DbError> {
    sqlx::query(
        "UPDATE schedules SET name=$2, prompt=$3, cron=$4, timezone=$5, gate_command=$6,
            gate_description=$7, enabled=$8, disabled_reason=$9, consecutive_failures=$10,
            next_fire_at=$11, fire_lease_until=NULL, updated_at=now()
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
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} {FROM} WHERE s.id = ANY($1) ORDER BY s.id"
    ))
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
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} {FROM} WHERE s.id = ANY($1) ORDER BY s.next_fire_at, s.id"
    ))
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

/// Disable with a reason; `next_fire_at` is cleared and the lease released.
pub async fn disable(
    conn: &mut PgConnection,
    id: Uuid,
    reason: &str,
    last_outcome: Option<&str>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE schedules SET enabled = false, disabled_reason = $2, next_fire_at = NULL,
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
    Ok(sqlx::query_as(&format!(
        "INSERT INTO schedule_runs (id, schedule_id, turn_id, outcome, fired_at)
         VALUES ($1,$2,$3,$4, now()) RETURNING {RUN_COLS}"
    ))
    .bind(Uuid::now_v7())
    .bind(schedule)
    .bind(turn)
    .bind(outcome)
    .fetch_one(conn)
    .await?)
}

pub async fn list_runs<'e, E: PgExecutor<'e>>(
    exec: E,
    schedule: Uuid,
    limit: i64,
) -> Result<Vec<RunRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {RUN_COLS} FROM schedule_runs WHERE schedule_id = $1
         ORDER BY fired_at DESC, id DESC LIMIT $2"
    ))
    .bind(schedule)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// The run row that carries `turn` (fenced writers lock it).
pub async fn run_by_turn(conn: &mut PgConnection, turn: Uuid) -> Result<Option<RunRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {RUN_COLS} FROM schedule_runs WHERE turn_id = $1 FOR UPDATE"
    ))
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
