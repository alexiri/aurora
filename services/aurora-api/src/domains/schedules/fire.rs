//! Firing (docs/10 §Schedules): due-work arm 9, run-now, gate results and run-outcome
//! bookkeeping. Everything runs inside the caller's transaction.

use super::cron;
use crate::app::AppState;
use crate::domains::conversations::service::{self as conv, NewMessage};
use crate::domains::hosted::lifecycle;
use aurora_core::{ApiError, limits};
use aurora_db::authz;
use aurora_db::repo::agents;
use aurora_db::repo::conversations::{self as convs, NewConversation};
use aurora_db::repo::schedules::{self as repo, RunRow, ScheduleRow};
use aurora_db::repo::turns::{self, Created, TurnRow};
use aurora_protocol::runner::GateOutcome;
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

/// Max schedules claimed per poll (bounded arm).
const ARM_LIMIT: i64 = 8;
/// Bound of gate output kept in the transcript notice.
const GATE_NOTICE_MAX: usize = 8 * 1024;

fn clip(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut e = max;
    while !s.is_char_boundary(e) {
        e -= 1;
    }
    &s[..e]
}

/// Prompt behind the header the model sees.
fn turn_text(s: &ScheduleRow) -> String {
    format!("Scheduled task: {}\n\n{}", s.name, s.prompt)
}

/// Append a notice to the schedule's origin conversation (if any).
async fn notify_origin(
    conn: &mut PgConnection,
    s: &ScheduleRow,
    code: &str,
    text: &str,
    turn: Option<Uuid>,
) -> Result<(), ApiError> {
    let Some(origin) = s.origin_conversation_id else {
        return Ok(());
    };
    if !convs::lock(&mut *conn, origin).await? {
        return Ok(());
    }
    conv::append_event(
        conn,
        origin,
        None,
        "notice",
        &json!({"code": code, "text": text, "schedule_id": s.id, "turn_id": turn}),
    )
    .await?;
    Ok(())
}

/// Outcome of trying to start a run.
struct Started {
    conversation: Uuid,
    run: RunRow,
}

/// Create the run conversation's turn and the run row. `None` when the conversation is busy.
async fn start_run(
    conn: &mut PgConnection,
    s: &ScheduleRow,
    agent: &agents::AgentRow,
) -> Result<Option<Started>, ApiError> {
    let ref_ = format!("schedule:{}", s.id);
    let (c, _) = convs::get_or_create(
        conn,
        &NewConversation {
            agent_id: agent.id,
            workspace_id: agent.workspace_id,
            kind: "schedule",
            external_ref: &ref_,
            direct_user_id: None,
            title: Some(&s.name),
            origin_conversation_id: s.origin_conversation_id,
        },
    )
    .await?;
    if !convs::lock(&mut *conn, c.id).await? {
        return Ok(None);
    }
    if turns::active_turn(&mut *conn, c.id).await?.is_some() {
        return Ok(None);
    }
    let text = turn_text(s);
    let door = json!({"kind": "schedule", "ref": s.id.to_string()});
    let created = conv::create_turn_tx(
        conn,
        &c,
        &NewMessage {
            door,
            speaker_kind: "system",
            speaker_user_id: None,
            speaker_label: "Schedule",
            text: &text,
            source_event_id: None,
            attachment_ids: &[],
        },
    )
    .await?;
    let turn = match created {
        Created::Turn(t) | Created::Followup(t) => t,
    };
    let run = repo::insert_run(conn, s.id, Some(turn.id), "fired").await?;
    Ok(Some(Started {
        conversation: c.id,
        run,
    }))
}

/// Arm 9: claim due schedules and fire each in its own savepoint.
pub async fn arm9(s: &AppState, tx: &mut PgConnection) -> Result<(), ApiError> {
    let claimed = repo::claim_due(tx, limits::SCHEDULE_FIRE_LEASE.as_secs_f64(), ARM_LIMIT).await?;
    for row in claimed {
        let mut sp = tx.begin().await.map_err(aurora_db::DbError::from)?;
        match fire_due(s, &mut sp, &row, Utc::now()).await {
            Ok(()) => sp.commit().await.map_err(aurora_db::DbError::from)?,
            Err(e) => {
                // Roll back this schedule only; its lease stays, so it is retried after expiry.
                tracing::error!(schedule = %row.id, error = %e.message(), "schedule fire failed");
                drop(sp);
            }
        }
    }
    Ok(())
}

async fn fire_due(
    s: &AppState,
    conn: &mut PgConnection,
    row: &ScheduleRow,
    now: DateTime<Utc>,
) -> Result<(), ApiError> {
    let _ = s;
    let Some(agent) = agents::get(&mut *conn, row.agent_id).await? else {
        return Ok(());
    };
    // Fire-time authority (04): the creator must still hold `can_use` on the workspace.
    // RFE-021: `anchor = workspace` is authorized by "an active owner exists" instead.
    let authorized = authz::automation_authorized(
        &mut *conn,
        &row.anchor,
        row.creator_user_id,
        agent.workspace_id,
    )
    .await?;
    if !authorized {
        repo::insert_run(conn, row.id, None, "skipped_unauthorized").await?;
        repo::disable(conn, row.id, "authorization", Some("failed")).await?;
        notify_origin(
            conn,
            row,
            "schedule_disabled",
            &format!(
                "Schedule \"{}\" was disabled: {}. Reassign it in the dashboard or recreate it.",
                row.name,
                if row.anchor == "workspace" {
                    "the workspace has no active owner"
                } else {
                    "its creator no longer has access to this agent"
                }
            ),
            None,
        )
        .await?;
        return Ok(());
    }
    // Next real occurrence after now; missed windows coalesce into this one late fire.
    let next = cron::next_after(&row.cron, &row.timezone, now).ok();
    if let Some(due) = row.next_fire_at
        && cron::next_after(&row.cron, &row.timezone, due).is_ok_and(|n| n <= now)
    {
        repo::insert_run(conn, row.id, None, "coalesced").await?;
    }
    match start_run(conn, row, &agent).await? {
        None => {
            repo::record_fire(conn, row.id, next, false, "skipped_busy", None).await?;
        }
        Some(st) => {
            repo::record_fire(conn, row.id, next, true, "ok", Some(st.conversation)).await?;
        }
    }
    Ok(())
}

/// `run-now`: fire immediately without moving the schedule. `None` when the run conversation is
/// busy (409 at the route).
pub async fn run_now(
    conn: &mut PgConnection,
    row: &ScheduleRow,
) -> Result<Option<RunRow>, ApiError> {
    let agent = agents::get(&mut *conn, row.agent_id)
        .await?
        .ok_or_else(super::service::not_found)?;
    let Some(st) = start_run(conn, row, &agent).await? else {
        return Ok(None);
    };
    repo::record_manual(conn, row.id, st.conversation).await?;
    Ok(Some(st.run))
}

/// Gate command to hand to the supervisor with `turn.deliver` (if the turn is a pending run).
pub async fn gate_for_turn(
    conn: &mut PgConnection,
    turn: Uuid,
) -> Result<Option<aurora_protocol::runner::Gate>, ApiError> {
    let cmd: Option<String> = sqlx::query_scalar(
        "SELECT s.gate_command FROM schedule_runs r JOIN schedules s ON s.id = r.schedule_id
         WHERE r.turn_id = $1 AND r.outcome = 'fired' AND s.gate_command IS NOT NULL",
    )
    .bind(turn)
    .fetch_optional(conn)
    .await
    .map_err(aurora_db::DbError::from)?
    .flatten();
    Ok(cmd.map(|command| aurora_protocol::runner::Gate {
        command,
        timeout_secs: u32::try_from(limits::GATE_TIMEOUT.as_secs()).unwrap_or(30),
    }))
}

/// Result of applying a `gate.result` report.
pub enum GateApplied {
    Applied,
    Duplicate,
    Invalid(&'static str),
}

/// `gate.result` (16 §W5): quiet ends the run silently, wake adds the output as a notice,
/// failed counts toward the failure disable. Fenced on the run outcome (I-25).
pub async fn gate_result(
    conn: &mut PgConnection,
    agent: Uuid,
    turn_id: Uuid,
    outcome: &GateOutcome,
    output: Option<&str>,
) -> Result<GateApplied, ApiError> {
    let Some(t) = turns::get(&mut *conn, turn_id)
        .await?
        .filter(|t| t.role == "turn" && t.agent_id == agent)
    else {
        return Ok(GateApplied::Invalid("unknown_turn"));
    };
    if t.door.get("kind").and_then(|k| k.as_str()) != Some("schedule") {
        return Ok(GateApplied::Invalid("not_a_schedule_turn"));
    }
    // Lock order: conversation, then run.
    convs::lock(&mut *conn, t.conversation_id).await?;
    let Some(run) = repo::run_by_turn(conn, turn_id).await? else {
        return Ok(GateApplied::Duplicate);
    };
    let Some(sched) = repo::get(&mut *conn, run.schedule_id).await? else {
        return Ok(GateApplied::Duplicate);
    };
    let out = output.unwrap_or("");
    match outcome {
        GateOutcome::Other(_) => return Ok(GateApplied::Invalid("unknown_outcome")),
        GateOutcome::Quiet => {
            if !repo::set_run_outcome(conn, run.id, "fired", "gated_quiet", true).await? {
                return Ok(GateApplied::Duplicate);
            }
            repo::reset_failures(conn, sched.id, "gated_quiet").await?;
            lifecycle::finish(conn, turn_id, "done", None, None, None).await?;
        }
        GateOutcome::Wake => {
            let seen: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM transcript_events WHERE conversation_id = $1
                   AND turn_id = $2 AND type = 'notice' AND payload->>'code' = 'gate_output')",
            )
            .bind(t.conversation_id)
            .bind(turn_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(aurora_db::DbError::from)?;
            if seen || run.outcome != "fired" {
                return Ok(GateApplied::Duplicate);
            }
            conv::append_event(
                conn,
                t.conversation_id,
                Some(turn_id),
                "notice",
                &json!({"code": "gate_output", "text": clip(out, GATE_NOTICE_MAX)}),
            )
            .await?;
        }
        GateOutcome::Failed => {
            if !repo::set_run_outcome(conn, run.id, "fired", "gate_failed", false).await? {
                return Ok(GateApplied::Duplicate);
            }
            let msg = if out.is_empty() {
                "The gate command failed."
            } else {
                clip(out, 1024)
            };
            lifecycle::fail(conn, turn_id, t.conversation_id, "gate_failed", msg).await?;
            record_failure(conn, &sched, turn_id, "gate failed").await?;
        }
    }
    Ok(GateApplied::Applied)
}

/// Count a failure; at `SCHEDULE_FAILURE_DISABLE` disable with `failures` and tell the origin.
async fn record_failure(
    conn: &mut PgConnection,
    sched: &ScheduleRow,
    turn: Uuid,
    what: &str,
) -> Result<(), ApiError> {
    let n = repo::add_failure(conn, sched.id).await?;
    let limit = i32::try_from(limits::SCHEDULE_FAILURE_DISABLE).unwrap_or(i32::MAX);
    if n >= limit {
        repo::disable(conn, sched.id, "failures", None).await?;
        notify_origin(
            conn,
            sched,
            "schedule_disabled",
            &format!(
                "Schedule \"{}\" was disabled after {n} consecutive failures.",
                sched.name
            ),
            Some(turn),
        )
        .await?;
    } else {
        notify_origin(
            conn,
            sched,
            "schedule_run",
            &format!("Scheduled task \"{}\" failed ({what}).", sched.name),
            Some(turn),
        )
        .await?;
    }
    Ok(())
}

/// Hook from `lifecycle::finish`: bookkeeping and the run report when a schedule turn ends.
pub async fn on_turn_finished(conn: &mut PgConnection, t: &TurnRow) -> Result<(), ApiError> {
    if t.door.get("kind").and_then(|k| k.as_str()) != Some("schedule") {
        return Ok(());
    }
    let Some(run) = repo::run_by_turn(conn, t.id).await? else {
        return Ok(());
    };
    // Gate failures and quiet runs were accounted for by `gate_result`.
    if run.outcome != "fired" {
        return Ok(());
    }
    let Some(sched) = repo::get(&mut *conn, run.schedule_id).await? else {
        return Ok(());
    };
    match t.status.as_str() {
        "done" => {
            repo::reset_failures(conn, sched.id, "ok").await?;
            notify_origin(
                conn,
                &sched,
                "schedule_run",
                &format!("Scheduled task \"{}\" finished.", sched.name),
                Some(t.id),
            )
            .await?;
        }
        "failed" => record_failure(conn, &sched, t.id, "turn failed").await?,
        _ => {}
    }
    Ok(())
}
