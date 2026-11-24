//! Schedule CRUD shared by the routes and the agent tools. Authorization is the caller's job.

use super::{cron, once};
use aurora_core::ApiError;
use aurora_db::repo::conversations as convs;
use aurora_db::repo::schedules::{self as repo, NewSchedule, ScheduleRow};
use aurora_db::repo::{agents::AgentRow, workspaces};
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

const NAME_MAX: usize = 120;
const PROMPT_MAX: usize = 20_000;
const GATE_COMMAND_MAX: usize = 2000;
const GATE_DESCRIPTION_MAX: usize = 200;
/// Schedules per agent.
const PER_AGENT_MAX: i64 = 50;

fn bad(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_field("Invalid schedule.", path, msg)
}

pub fn not_found() -> ApiError {
    ApiError::not_found("Schedule not found.")
}

pub struct Create {
    pub name: String,
    pub prompt: String,
    /// Exactly one of `cron` / `run_at` (P5-SCHED-ONCE, C-1092).
    pub cron: Option<String>,
    pub run_at: Option<String>,
    pub timezone: Option<String>,
    pub gate_command: Option<String>,
    pub gate_description: Option<String>,
    /// `creator` or `workspace` (RFE-021); tools always pass `creator`.
    pub anchor: &'static str,
}

/// (P6-TPL-3) Pause a freshly created schedule inside the caller's transaction: the
/// from-template create makes a schedule the person switched off in the four steps (the same
/// write `patch` does — `enabled = false`, a user pause, nothing armed), without opening its
/// own transaction.
pub async fn pause_in_tx(
    tx: &mut PgConnection,
    mut r: ScheduleRow,
    paused_by: Uuid,
) -> Result<ScheduleRow, ApiError> {
    r.enabled = false;
    r.disabled_reason = Some("user".into());
    r.disabled_by = Some(paused_by);
    r.next_fire_at = None;
    repo::update(tx, &r, true).await.map_err(Into::into)
}

#[derive(Default)]
pub struct Patch {
    pub name: Option<String>,
    pub prompt: Option<String>,
    pub cron: Option<String>,
    /// (P5-SCHED-ONCE) Makes the schedule one-time; at most one of `cron` / `run_at`.
    pub run_at: Option<String>,
    pub timezone: Option<String>,
    pub gate_command: Option<Option<String>>,
    pub gate_description: Option<Option<String>>,
    pub enabled: Option<bool>,
    /// (C-1096) The person pausing (recorded with a user pause); null for an agent's tool call.
    pub paused_by: Option<Uuid>,
}

fn both_kinds() -> ApiError {
    bad("/run_at", "set either cron or run_at, not both")
}

/// (C-1092) This schedule has already run (Done): `enabled` cannot change it.
pub fn completed() -> ApiError {
    ApiError::conflict(
        "schedule_completed",
        "This schedule has already run. Set a new time to run it again.",
    )
}

/// The instant to arm a schedule at when it is (re-)enabled: a cron's next occurrence; a pending
/// one-time schedule's `run_at`, or now when that has passed (late rather than lost, D-421);
/// `None` for a Done one (it stays Done).
pub fn arm_at(r: &ScheduleRow, now: DateTime<Utc>) -> Result<Option<DateTime<Utc>>, ApiError> {
    match (&r.cron, r.run_at) {
        (Some(c), _) => cron::validate(c, &r.timezone, now).map(Some),
        (None, Some(_)) if r.completed_at.is_some() => Ok(None),
        (None, Some(at)) => Ok(Some(at.max(now))),
        (None, None) => Ok(None),
    }
}

/// A new schedule's timing: `(cron, run_at, first fire)`.
type Timing = (Option<String>, Option<DateTime<Utc>>, DateTime<Utc>);

/// The timing of a new schedule: the cron and its first occurrence, or the one-time instant.
fn timing(
    cron_expr: Option<&str>,
    run_at: Option<&str>,
    tz: &str,
    now: DateTime<Utc>,
) -> Result<Timing, ApiError> {
    match (cron_expr, run_at) {
        (Some(_), Some(_)) => Err(both_kinds()),
        (None, None) => Err(bad("/cron", "set either cron or run_at")),
        (Some(c), None) => {
            let c = c.trim().to_owned();
            let next = cron::validate(&c, tz, now)?;
            Ok((Some(c), None, next))
        }
        (None, Some(at)) => {
            let at = once::validate(at, tz, now)?;
            Ok((None, Some(at), at))
        }
    }
}

fn check_name(v: &str) -> Result<(), ApiError> {
    aurora_core::names::clean(v, NAME_MAX)
        .map(|_| ())
        .map_err(|e| bad("/name", &e.to_string()))
}

fn check_prompt(v: &str) -> Result<(), ApiError> {
    if v.trim().is_empty() || v.chars().count() > PROMPT_MAX {
        return Err(bad("/prompt", "1 to 20000 characters"));
    }
    Ok(())
}

fn check_gate(cmd: &Option<String>, desc: &Option<String>) -> Result<(), ApiError> {
    if let Some(c) = cmd
        && (c.trim().is_empty() || c.chars().count() > GATE_COMMAND_MAX)
    {
        return Err(bad("/gate_command", "1 to 2000 characters, or null"));
    }
    if let Some(d) = desc
        && d.chars().count() > GATE_DESCRIPTION_MAX
    {
        return Err(bad("/gate_description", "at most 200 characters"));
    }
    Ok(())
}

pub async fn list(pool: &PgPool, agent: Uuid) -> Result<Vec<ScheduleRow>, ApiError> {
    Ok(repo::list_for_agent(pool, agent).await?)
}

pub async fn get(pool: &PgPool, id: Uuid) -> Result<ScheduleRow, ApiError> {
    repo::get(pool, id).await?.ok_or_else(not_found)
}

pub async fn create(
    pool: &PgPool,
    agent: &AgentRow,
    c: Create,
    creator: Option<Uuid>,
    origin: Option<Uuid>,
) -> Result<ScheduleRow, ApiError> {
    let mut tx = pool.begin().await.map_err(aurora_db::DbError::from)?;
    let row = create_in_tx(&mut tx, agent, c, creator, origin).await?;
    tx.commit().await.map_err(aurora_db::DbError::from)?;
    Ok(row)
}

/// The transaction body of [`create`] (P6-TPL-3's from-template route calls it inside its own
/// bigger transaction, with `creator` the person who created the agent — the fire-time
/// authority, `10 §Schedules`). Everything is validated before the row is inserted.
pub async fn create_in_tx(
    tx: &mut PgConnection,
    agent: &AgentRow,
    c: Create,
    creator: Option<Uuid>,
    origin: Option<Uuid>,
) -> Result<ScheduleRow, ApiError> {
    check_name(&c.name)?;
    check_prompt(&c.prompt)?;
    check_gate(&c.gate_command, &c.gate_description)?;
    let tz = match c.timezone {
        Some(t) => t,
        None => workspaces::get(&mut *tx, agent.workspace_id)
            .await?
            .map_or_else(|| "UTC".to_owned(), |w| w.timezone),
    };
    let (cron_expr, run_at, next) =
        timing(c.cron.as_deref(), c.run_at.as_deref(), &tz, Utc::now())?;
    // Serialize the per-agent cap.
    sqlx::query("SELECT 1 FROM agents WHERE id = $1 FOR UPDATE")
        .bind(agent.id)
        .execute(&mut *tx)
        .await
        .map_err(aurora_db::DbError::from)?;
    if repo::count_for_agent(&mut *tx, agent.id).await? >= PER_AGENT_MAX {
        return Err(ApiError::conflict(
            "schedule_limit_reached",
            "This agent has reached its schedule limit.",
        ));
    }
    repo::insert(
        tx,
        &NewSchedule {
            agent_id: agent.id,
            name: &c.name,
            prompt: &c.prompt,
            cron: cron_expr.as_deref(),
            run_at,
            timezone: &tz,
            gate_command: c.gate_command.as_deref(),
            gate_description: c.gate_description.as_deref(),
            creator_user_id: creator,
            anchor: c.anchor,
            origin_conversation_id: origin,
            next_fire_at: Some(next),
        },
    )
    .await
    .map_err(ApiError::from)
}

pub async fn patch(pool: &PgPool, seen: ScheduleRow, p: Patch) -> Result<ScheduleRow, ApiError> {
    patch_with(pool, seen, p, async {}).await
}

/// [`patch`] with a hook that runs after the caller's read and before the row lock is taken
/// (tests interleave a fire there).
pub async fn patch_with(
    pool: &PgPool,
    seen: ScheduleRow,
    p: Patch,
    between: impl std::future::Future<Output = ()>,
) -> Result<ScheduleRow, ApiError> {
    between.await;
    // (FIX-110) Start from the locked current row, not the caller's possibly stale read.
    let mut tx = pool.begin().await.map_err(aurora_db::DbError::from)?;
    // (P5-REFINE-7, C-1074) The run threads follow the name. Written before the schedule row is
    // locked: the lock order is conversation, then schedule (a finishing run turn holds its
    // thread and then touches the schedule).
    if let Some(v) = &p.name {
        check_name(v)?;
        convs::rename_automation_threads(&mut tx, "schedule", seen.id, v).await?;
    }
    let mut r = repo::get_for_update(&mut tx, seen.id)
        .await?
        .ok_or_else(not_found)?;
    let mut reschedule = false;
    if let Some(v) = p.name {
        check_name(&v)?;
        r.name = v;
    }
    if let Some(v) = p.prompt {
        check_prompt(&v)?;
        r.prompt = v;
    }
    if let Some(v) = p.gate_command {
        r.gate_command = v;
    }
    if let Some(v) = p.gate_description {
        r.gate_description = v;
    }
    check_gate(&r.gate_command, &r.gate_description)?;
    let now = Utc::now();
    if p.cron.is_some() && p.run_at.is_some() {
        return Err(both_kinds());
    }
    // The timing after this patch; `Some(next)` when it changed (re-armed if enabled).
    let mut retimed: Option<Option<DateTime<Utc>>> = None;
    let old_tz = std::mem::take(&mut r.timezone);
    r.timezone = p.timezone.clone().unwrap_or_else(|| old_tz.clone());
    if let Some(v) = p.cron {
        // (C-1092) A cron makes the schedule recurring.
        let c = v.trim().to_owned();
        retimed = Some(Some(cron::validate(&c, &r.timezone, now)?));
        r.cron = Some(c);
        r.run_at = None;
        r.completed_at = None;
    } else if let Some(v) = p.run_at {
        // (C-1092) A run_at makes it one-time and re-arms a Done one.
        let at = once::validate(&v, &r.timezone, now)?;
        retimed = Some(Some(at));
        r.cron = None;
        r.run_at = Some(at);
        r.completed_at = None;
    } else if p.timezone.is_some() {
        match (&r.cron, r.run_at) {
            (Some(c), _) => retimed = Some(Some(cron::validate(c, &r.timezone, now)?)),
            (None, Some(at)) => {
                // A one-time schedule keeps its local wall time in the new zone.
                let moved = once::rebase(at, &old_tz, &r.timezone)?;
                r.run_at = Some(moved);
                if r.completed_at.is_none() {
                    once::check_bounds(moved, now)?;
                    retimed = Some(Some(moved));
                }
            }
            (None, None) => {}
        }
    }
    if p.enabled.is_some() && r.completed_at.is_some() {
        return Err(completed());
    }
    match p.enabled {
        Some(true) if !r.enabled => {
            r.enabled = true;
            r.disabled_reason = None;
            r.disabled_by = None;
            r.consecutive_failures = 0;
            r.next_fire_at = match retimed {
                Some(next) => next,
                None => arm_at(&r, now)?,
            };
            reschedule = true;
        }
        Some(false) if r.enabled => {
            r.enabled = false;
            r.disabled_reason = Some("user".into());
            r.disabled_by = p.paused_by;
            r.next_fire_at = None;
            reschedule = true;
        }
        _ => {}
    }
    if let Some(next) = retimed {
        if r.enabled {
            r.next_fire_at = next;
        }
        reschedule = true;
    }
    let row = repo::update(&mut tx, &r, reschedule).await?;
    tx.commit().await.map_err(aurora_db::DbError::from)?;
    Ok(row)
}

/// Delete a schedule and its run log; (P5-REFINE-7, C-1074) its run threads stay, marked
/// deleted in the same transaction.
pub async fn delete(pool: &PgPool, id: Uuid) -> Result<(), ApiError> {
    let mut tx = pool.begin().await.map_err(aurora_db::DbError::from)?;
    if !delete_tx(&mut tx, id).await? {
        return Err(not_found());
    }
    tx.commit().await.map_err(aurora_db::DbError::from)?;
    Ok(())
}

/// The delete's statements in the caller's transaction (threads marked first: the lock order is
/// conversation, then schedule). False when the schedule is gone.
async fn delete_tx(conn: &mut PgConnection, id: Uuid) -> Result<bool, aurora_db::DbError> {
    convs::mark_automation_deleted(&mut *conn, "schedule", id).await?;
    repo::delete(conn, id).await
}

/// (P5-SCHED-ONCE, C-1092) One batch of the daily retention pass: Done one-time schedules older
/// than `SCHEDULE_DONE_RETENTION`, each deleted through [`delete`]'s path (its run threads are
/// marked as from a deleted automation). Returns how many were deleted.
pub async fn prune_done(pool: &PgPool, batch: i64) -> Result<u64, aurora_db::DbError> {
    let age = aurora_core::limits::SCHEDULE_DONE_RETENTION.as_secs_f64();
    let mut n = 0;
    for id in repo::done_before(pool, age, batch).await? {
        let mut tx = pool.begin().await?;
        let gone = delete_tx(&mut tx, id).await?;
        tx.commit().await?;
        n += u64::from(gone);
    }
    Ok(n)
}
