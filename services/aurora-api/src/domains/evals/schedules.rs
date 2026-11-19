//! Eval schedules (P4-EVAL-2; C-870, D-293): CRUD for `can_use` callers and the fire arm. A
//! schedule fires through the same lease, anchor and fire-time authority as the schedules engine
//! (`04 §Fire-time authority`) but starts an eval run. Cadence is at least
//! `EVAL_SCHEDULE_MIN_INTERVAL`; the failure streak lives in `engine::after_finish`.

use super::dto::{CreateEvalSchedule, EvalSchedule, EvalScheduleList, PatchEvalSchedule};
use super::runs::{run_audit, start_tx};
use super::service::{current_fingerprint, visible_agent};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent};
use crate::domains::schedules::cron;
use crate::http::auth::{Caller, require_own};
use aurora_core::{ApiError, limits};
use aurora_db::DbError;
use aurora_db::authz;
use aurora_db::repo::agents::AgentRow;
use aurora_db::repo::evals::{self as repo, NewSchedule, ScheduleRow};
use aurora_db::repo::{agents, freeze};
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

const ARM_LIMIT: i64 = 8;

fn invalid(field: &str, msg: &str) -> ApiError {
    ApiError::unprocessable("invalid", msg).with_details(json!({ "field": field }))
}

fn schedule_404() -> ApiError {
    ApiError::not_found("Eval schedule not found.")
}

fn min_interval() -> Duration {
    Duration::seconds(i64::try_from(limits::EVAL_SCHEDULE_MIN_INTERVAL.as_secs()).unwrap_or(3600))
}

/// The first occurrence after now, enforcing `EVAL_SCHEDULE_MIN_INTERVAL` (422 `invalid`).
fn validate(cron_expr: &str, tz: &str) -> Result<DateTime<Utc>, ApiError> {
    cron::validate_min(cron_expr, tz, Utc::now(), min_interval()).map_err(|e| {
        let field = if e
            .details()
            .is_some_and(|d| d.to_string().contains("/timezone"))
        {
            "timezone"
        } else {
            "cron"
        };
        let msg = e
            .details()
            .and_then(|d| d.pointer("/fields/0/message"))
            .and_then(|m| m.as_str())
            .unwrap_or("invalid schedule");
        invalid(field, msg)
    })
}

fn check_name(n: &str) -> Result<(), ApiError> {
    aurora_core::names::clean(n, 200)
        .map(|_| ())
        .map_err(|e| invalid("name", &e.to_string()))
}

fn sched_audit(
    c: &Caller,
    a: &AgentRow,
    id: Uuid,
    action: &str,
    meta: serde_json::Value,
) -> AuditEvent {
    AuditEvent::by(c, action)
        .org(a.org_id)
        .workspace(a.workspace_id)
        .target("eval_schedule", id)
        .metadata(meta)
}

pub async fn list(s: &AppState, c: &Caller, agent: Uuid) -> Result<EvalScheduleList, ApiError> {
    let a = visible_agent(s, c, agent).await?;
    let rows = repo::list_schedules(&s.pool, a.id).await?;
    Ok(EvalScheduleList {
        schedules: rows
            .into_iter()
            .map(|r| EvalSchedule::new(r, &a.name))
            .collect(),
    })
}

pub async fn create(
    s: &AppState,
    c: &Caller,
    agent: Uuid,
    b: CreateEvalSchedule,
) -> Result<EvalSchedule, ApiError> {
    let a = visible_agent(s, c, agent).await?;
    check_name(&b.name)?;
    let anchor = b.anchor.as_deref().unwrap_or("creator");
    if !matches!(anchor, "creator" | "workspace") {
        return Err(invalid("anchor", "creator or workspace"));
    }
    if anchor == "workspace" {
        require_own(&c.workspace(s, a.workspace_id).await?)?;
    }
    let next = validate(&b.cron, &b.timezone)?;
    let enabled = b.enabled.unwrap_or(true);
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    // Serialise the cap with the question list's lock (one lock per agent).
    repo::lock_agent_questions(&mut tx, a.id).await?;
    if repo::count_schedules(&mut *tx, a.id).await? >= limits::EVAL_SCHEDULES_PER_AGENT as i64 {
        return Err(ApiError::unprocessable(
            "eval_schedule_limit",
            "This agent has the most eval schedules it can have.",
        )
        .with_details(json!({ "limit": limits::EVAL_SCHEDULES_PER_AGENT })));
    }
    let id = repo::insert_schedule(
        &mut tx,
        &NewSchedule {
            agent_id: a.id,
            name: b.name.trim(),
            cron: &b.cron,
            timezone: &b.timezone,
            enabled,
            creator: Some(c.user_id),
            anchor,
            next_fire_at: enabled.then_some(next),
        },
    )
    .await?;
    let row = repo::get_schedule(&mut *tx, id)
        .await?
        .ok_or_else(schedule_404)?;
    tx.commit().await.map_err(DbError::from)?;
    audit::record(
        s,
        sched_audit(
            c,
            &a,
            id,
            "eval.schedule.create",
            json!({"agent_id": a.id, "anchor": anchor}),
        ),
    );
    Ok(EvalSchedule::new(row, &a.name))
}

async fn load(s: &AppState, c: &Caller, id: Uuid) -> Result<(ScheduleRow, AgentRow), ApiError> {
    let r = repo::get_schedule(&s.pool, id)
        .await?
        .ok_or_else(schedule_404)?;
    let a = visible_agent(s, c, r.agent_id).await.map_err(|e| {
        if e.status() == http::StatusCode::NOT_FOUND {
            schedule_404()
        } else {
            e
        }
    })?;
    Ok((r, a))
}

pub async fn get(s: &AppState, c: &Caller, id: Uuid) -> Result<EvalSchedule, ApiError> {
    let (r, a) = load(s, c, id).await?;
    Ok(EvalSchedule::new(r, &a.name))
}

pub async fn patch(
    s: &AppState,
    c: &Caller,
    id: Uuid,
    b: PatchEvalSchedule,
) -> Result<EvalSchedule, ApiError> {
    let (r, a) = load(s, c, id).await?;
    if let Some(n) = b.name.as_deref() {
        check_name(n)?;
    }
    let cron_expr = b.cron.as_deref().unwrap_or(&r.cron);
    let tz = b.timezone.as_deref().unwrap_or(&r.timezone);
    let changed = b.cron.is_some() || b.timezone.is_some();
    let enable = b.enabled;
    let will_run = enable.unwrap_or(r.enabled);
    // The cadence is checked whenever the cron changes or the schedule is switched on.
    let next = (changed || enable == Some(true))
        .then(|| validate(cron_expr, tz))
        .transpose()?;
    let next_fire = match (will_run, next) {
        (false, _) => Some(None),
        (true, Some(n)) => Some(Some(n)),
        (true, None) => None,
    };
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    repo::update_schedule(
        &mut tx,
        id,
        b.name.as_deref().map(str::trim),
        changed.then_some((cron_expr, tz)),
        enable,
        next_fire,
    )
    .await?;
    let row = repo::get_schedule(&mut *tx, id)
        .await?
        .ok_or_else(schedule_404)?;
    tx.commit().await.map_err(DbError::from)?;
    audit::record(
        s,
        sched_audit(
            c,
            &a,
            id,
            "eval.schedule.update",
            json!({"agent_id": a.id, "enabled": row.enabled}),
        ),
    );
    Ok(EvalSchedule::new(row, &a.name))
}

pub async fn delete(s: &AppState, c: &Caller, id: Uuid) -> Result<(), ApiError> {
    let (_, a) = load(s, c, id).await?;
    let mut tx = s.pool.begin().await.map_err(DbError::from)?;
    repo::delete_schedule(&mut tx, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    audit::record(
        s,
        sched_audit(c, &a, id, "eval.schedule.delete", json!({"agent_id": a.id})),
    );
    Ok(())
}

// ---- the fire arm ----

/// Arm: claim due eval schedules and fire each in its own savepoint (`SCHEDULE_FIRE_LEASE`).
pub async fn arm(s: &AppState, tx: &mut PgConnection) -> Result<(), ApiError> {
    let claimed =
        repo::claim_due_schedules(tx, limits::SCHEDULE_FIRE_LEASE.as_secs_f64(), ARM_LIMIT).await?;
    for row in claimed {
        let mut sp = tx.begin().await.map_err(DbError::from)?;
        match fire(s, &mut sp, &row, Utc::now()).await {
            Ok(()) => sp.commit().await.map_err(DbError::from)?,
            Err(e) => {
                tracing::error!(schedule = %row.id, code = e.code(), "eval schedule fire failed");
                drop(sp);
            }
        }
    }
    Ok(())
}

async fn fire(
    s: &AppState,
    conn: &mut PgConnection,
    row: &ScheduleRow,
    now: DateTime<Utc>,
) -> Result<(), ApiError> {
    let Some(agent) = agents::get(&mut *conn, row.agent_id).await? else {
        return Ok(());
    };
    let next = cron::next_after(&row.cron, &row.timezone, now).ok();
    // A frozen agent is neither fired nor disabled for authorization; the next occurrence is
    // armed (missed fires coalesce).
    if freeze::is_agent_frozen(&mut *conn, agent.id).await? {
        repo::record_schedule_fire(conn, row.id, next, false, "skipped_frozen", None).await?;
        return Ok(());
    }
    // Fire-time authority (04): the anchor decides, exactly as for ordinary schedules.
    if !authz::automation_authorized(conn, &row.anchor, row.creator_user_id, agent.workspace_id)
        .await?
    {
        repo::disable_schedule(conn, row.id, "authorization", Some("failed")).await?;
        return Ok(());
    }
    if repo::active_run(&mut *conn, agent.id).await?.is_some()
        || repo::runs_started_today(&mut *conn, agent.id).await?
            >= limits::EVAL_RUNS_PER_AGENT_DAY as i64
    {
        repo::record_schedule_fire(conn, row.id, next, false, "skipped_busy", None).await?;
        return Ok(());
    }
    let fp = current_fingerprint(s, &agent).await?;
    match start_tx(conn, &agent, &fp, "schedule", Some(row.id), None).await {
        Ok(run) => {
            repo::record_schedule_fire(conn, row.id, next, true, "ok", Some(run)).await?;
            let ev = run_audit(
                AuditActor::system("evals"),
                &agent,
                run,
                "eval.run.start",
                json!({"agent_id": agent.id, "fingerprint_hash": fp.fingerprint_hash,
                       "trigger": "schedule", "schedule_id": row.id}),
            );
            audit::insert_tx(conn, &ev).await?;
        }
        Err(e) if e.code() == "eval_no_questions" => {
            // Nothing to run: counts as a failed fire toward the streak.
            repo::record_schedule_fire(conn, row.id, next, false, "failed", None).await?;
            let n = repo::schedule_run_outcome(conn, row.id, true).await?;
            if n >= i32::try_from(limits::SCHEDULE_FAILURE_DISABLE).unwrap_or(i32::MAX) {
                repo::disable_schedule(conn, row.id, "failures", None).await?;
            }
        }
        Err(e) if matches!(e.code(), "eval_run_active" | "eval_runs_limited") => {
            repo::record_schedule_fire(conn, row.id, next, false, "skipped_busy", None).await?;
        }
        Err(e) if e.code() == "frozen" => {
            repo::record_schedule_fire(conn, row.id, next, false, "skipped_frozen", None).await?;
        }
        Err(e) => return Err(e),
    }
    Ok(())
}
