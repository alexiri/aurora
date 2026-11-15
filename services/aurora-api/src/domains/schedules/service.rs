//! Schedule CRUD shared by the routes and the agent tools. Authorization is the caller's job.

use super::cron;
use aurora_core::{ApiError, FieldError};
use aurora_db::repo::schedules::{self as repo, NewSchedule, ScheduleRow};
use aurora_db::repo::{agents::AgentRow, workspaces};
use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

const NAME_MAX: usize = 120;
const PROMPT_MAX: usize = 20_000;
const GATE_COMMAND_MAX: usize = 2000;
const GATE_DESCRIPTION_MAX: usize = 200;
/// Schedules per agent.
const PER_AGENT_MAX: i64 = 50;

fn bad(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_request("Invalid schedule.", vec![FieldError::new(path, msg)])
}

pub fn not_found() -> ApiError {
    ApiError::not_found("Schedule not found.")
}

pub struct Create {
    pub name: String,
    pub prompt: String,
    pub cron: String,
    pub timezone: Option<String>,
    pub gate_command: Option<String>,
    pub gate_description: Option<String>,
    /// `creator` or `workspace` (RFE-021); tools always pass `creator`.
    pub anchor: &'static str,
}

#[derive(Default)]
pub struct Patch {
    pub name: Option<String>,
    pub prompt: Option<String>,
    pub cron: Option<String>,
    pub timezone: Option<String>,
    pub gate_command: Option<Option<String>>,
    pub gate_description: Option<Option<String>>,
    pub enabled: Option<bool>,
}

fn check_name(v: &str) -> Result<(), ApiError> {
    if v.trim().is_empty() || v.chars().count() > NAME_MAX {
        return Err(bad("/name", "1 to 120 characters"));
    }
    Ok(())
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
    check_name(&c.name)?;
    check_prompt(&c.prompt)?;
    check_gate(&c.gate_command, &c.gate_description)?;
    let tz = match c.timezone {
        Some(t) => t,
        None => workspaces::get(pool, agent.workspace_id)
            .await?
            .map_or_else(|| "UTC".to_owned(), |w| w.timezone),
    };
    let next = cron::validate(&c.cron, &tz, Utc::now())?;
    let mut tx = pool.begin().await.map_err(aurora_db::DbError::from)?;
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
    let row = repo::insert(
        &mut tx,
        &NewSchedule {
            agent_id: agent.id,
            name: &c.name,
            prompt: &c.prompt,
            cron: c.cron.trim(),
            timezone: &tz,
            gate_command: c.gate_command.as_deref(),
            gate_description: c.gate_description.as_deref(),
            creator_user_id: creator,
            anchor: c.anchor,
            origin_conversation_id: origin,
            next_fire_at: Some(next),
        },
    )
    .await?;
    tx.commit().await.map_err(aurora_db::DbError::from)?;
    Ok(row)
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
    let timing = p.cron.is_some() || p.timezone.is_some();
    if let Some(v) = p.cron {
        r.cron = v.trim().to_owned();
    }
    if let Some(v) = p.timezone {
        r.timezone = v;
    }
    match p.enabled {
        Some(true) if !r.enabled => {
            r.enabled = true;
            r.disabled_reason = None;
            r.consecutive_failures = 0;
            r.next_fire_at = Some(cron::validate(&r.cron, &r.timezone, Utc::now())?);
            reschedule = true;
        }
        Some(false) if r.enabled => {
            r.enabled = false;
            r.disabled_reason = Some("user".into());
            r.next_fire_at = None;
            reschedule = true;
        }
        _ => {}
    }
    if timing {
        let next = cron::validate(&r.cron, &r.timezone, Utc::now())?;
        if r.enabled {
            r.next_fire_at = Some(next);
        }
        reschedule = true;
    }
    let row = repo::update(&mut tx, &r, reschedule).await?;
    tx.commit().await.map_err(aurora_db::DbError::from)?;
    Ok(row)
}

pub async fn delete(pool: &PgPool, id: Uuid) -> Result<(), ApiError> {
    let mut conn = pool.acquire().await.map_err(aurora_db::DbError::from)?;
    if repo::delete(&mut conn, id).await? {
        Ok(())
    } else {
        Err(not_found())
    }
}
