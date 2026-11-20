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
    &s[..s.floor_char_boundary(max)]
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
    // (P5-SCHED-ONCE, SEC-144) A schedule writes into a person's own chat only while it is that
    // person's schedule: after a reassignment the old owner's direct chat gets nothing. (A
    // creator-less schedule names nobody else: a deleted creator's direct chats are gone too.)
    if let Some(c) = convs::get(&mut *conn, origin).await?
        && c.kind == "direct"
        && s.creator_user_id.is_some()
        && c.direct_user_id != s.creator_user_id
    {
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

/// The schedule door's message: the prompt behind the header for the agent, and (C-1079) the
/// `display` people read (the schedule's name, not the prompt header).
fn schedule_message<'a>(
    s: &ScheduleRow,
    run_id: Uuid,
    text: &'a str,
) -> (NewMessage<'a>, serde_json::Value) {
    let door = json!({"kind": "schedule", "ref": s.id.to_string()});
    let display =
        json!({"kind": "schedule", "name": s.name, "schedule_id": s.id, "run_id": run_id});
    (
        NewMessage {
            door,
            speaker_kind: "system",
            speaker_user_id: None,
            speaker_label: "Schedule",
            text,
            source_event_id: None,
            attachment_ids: &[],
            reply_to_seq: None,
        },
        display,
    )
}

/// Start a run in its own thread (P5-REFINE-7, C-1074): `schedule:<schedule>:<run>`, with the
/// schedule's origin so the FIX-192 mirror still finds the origin's thread link. `None` when the
/// schedule is busy: its latest run's turn is still active (the busy rule, decided under the
/// schedule row lock so a fire and Run now decide one at a time). (P5-SCHED-ONCE) A one-time
/// schedule's fire passes `busy_rule = false`: a single run is never skipped as busy.
async fn start_run(
    conn: &mut PgConnection,
    s: &ScheduleRow,
    agent: &agents::AgentRow,
    busy_rule: bool,
) -> Result<Option<Started>, ApiError> {
    if !repo::lock(&mut *conn, s.id).await?
        || (busy_rule && repo::latest_run_active(&mut *conn, s.id).await?)
    {
        return Ok(None);
    }
    let run_id = Uuid::now_v7();
    let ref_ = format!("schedule:{}:{run_id}", s.id);
    let (c, _) = convs::get_or_create_in(
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
        Some(convs::Automation {
            kind: "schedule",
            id: s.id,
        }),
    )
    .await?;
    let text = turn_text(s);
    let (msg, display) = schedule_message(s, run_id, &text);
    let created = conv::create_turn_tx_with(conn, &c, &msg, Some(&display)).await?;
    let turn = match created {
        Created::Turn(t) | Created::Followup(t) => t,
    };
    let run = repo::insert_run_with_id(conn, run_id, s.id, Some(turn.id), "fired").await?;
    Ok(Some(Started {
        conversation: c.id,
        run,
    }))
}

/// (P5-SCHED-ONCE, D-422, C-1093, SEC-144) Start a one-time run in its origin conversation, the
/// direct chat of the schedule's current creator ([`repo::origin_for_run`] checked it). It queues
/// behind an active turn there as a follow-up (never steered into it, never skipped); when the
/// follow-up is promoted the run follows its turn ([`on_followup_promoted`]).
async fn start_in_origin(
    conn: &mut PgConnection,
    s: &ScheduleRow,
    origin: Uuid,
) -> Result<Option<Started>, ApiError> {
    let Some(c) = convs::get(&mut *conn, origin).await? else {
        return Ok(None);
    };
    let run_id = Uuid::now_v7();
    let text = turn_text(s);
    let (msg, display) = schedule_message(s, run_id, &text);
    let created = conv::create_turn_tx_with(conn, &c, &msg, Some(&display)).await?;
    let turn = match created {
        Created::Turn(t) | Created::Followup(t) => t,
    };
    let run = repo::insert_run_with_id(conn, run_id, s.id, Some(turn.id), "fired").await?;
    Ok(Some(Started {
        conversation: c.id,
        run,
    }))
}

/// (P5-SCHED-ONCE) A schedule run that waited as a follow-up became its own turn: the run now
/// points at that turn (its result, gate and push follow it). Returns the `display` of the new
/// `turn.queued` event (C-1079), so the promoted run still reads as the schedule's name.
pub async fn on_followup_promoted(
    conn: &mut PgConnection,
    from: Uuid,
    to: Uuid,
) -> Result<Option<serde_json::Value>, ApiError> {
    let row: Option<(Uuid, Uuid, String)> = sqlx::query_as(
        "UPDATE schedule_runs r SET turn_id = $2 FROM schedules s
         WHERE r.turn_id = $1 AND s.id = r.schedule_id
         RETURNING r.id, s.id, s.name",
    )
    .bind(from)
    .bind(to)
    .fetch_optional(conn)
    .await
    .map_err(aurora_db::DbError::from)?;
    Ok(row.map(|(run_id, schedule_id, name)| {
        json!({"kind": "schedule", "name": name, "schedule_id": schedule_id, "run_id": run_id})
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
    // (phase 2, RFE-006, FU-P2-021a) A frozen agent is not fired, whatever the anchor: the
    // schedule is neither disabled nor sent through the authority check; `next_fire_at` advances
    // (missed fires coalesce, no catch-up burst) and a `skipped_frozen` run is written. Checked
    // before the fire-time authority so a freeze never disables a schedule for authorization.
    if aurora_db::repo::freeze::is_agent_frozen(&mut *conn, agent.id).await? {
        let Some(expr) = row.cron.as_deref() else {
            // (P5-SCHED-ONCE, C-1092) A one-time schedule waits for the paused agent, late rather
            // than lost: one `waiting_paused` run, checked again every `SCHEDULE_ONCE_RECHECK`.
            let recheck = chrono::Duration::from_std(limits::SCHEDULE_ONCE_RECHECK)
                .unwrap_or(chrono::Duration::seconds(60));
            if !repo::wait_paused(conn, row.id, now + recheck).await? {
                repo::insert_run(conn, row.id, None, "waiting_paused").await?;
            }
            return Ok(());
        };
        let next = cron::next_after(expr, &row.timezone, now).ok();
        repo::insert_run(conn, row.id, None, "skipped_frozen").await?;
        repo::record_fire(conn, row.id, next, false, "skipped_frozen", None).await?;
        return Ok(());
    }
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
    let Some(expr) = row.cron.as_deref() else {
        return fire_once(conn, row, &agent).await;
    };
    // Next real occurrence after now; missed windows coalesce into this one late fire.
    let next = cron::next_after(expr, &row.timezone, now).ok();
    if let Some(due) = row.next_fire_at
        && cron::next_after(expr, &row.timezone, due).is_ok_and(|n| n <= now)
    {
        repo::insert_run(conn, row.id, None, "coalesced").await?;
    }
    match start_run(conn, row, &agent, true).await? {
        None => {
            repo::record_fire(conn, row.id, next, false, "skipped_busy", None).await?;
        }
        Some(st) => {
            repo::record_fire(conn, row.id, next, true, "ok", Some(st.conversation)).await?;
        }
    }
    Ok(())
}

/// (P5-SCHED-ONCE, D-421, D-422, C-1092, C-1093) Fire a due one-time schedule: in the creator's
/// own chat when [`repo::origin_for_run`] allows it (SEC-144), else in its own run thread; never
/// skipped as busy, never coalesced. Done in the same transaction.
async fn fire_once(
    conn: &mut PgConnection,
    row: &ScheduleRow,
    agent: &agents::AgentRow,
) -> Result<(), ApiError> {
    let started = match repo::origin_for_run(&mut *conn, row).await? {
        Some(origin) => start_in_origin(conn, row, origin).await?,
        None => None,
    };
    let started = match started {
        Some(st) => Some(st),
        None => start_run(conn, row, agent, false).await?,
    };
    repo::complete_once(conn, row.id, started.map(|st| st.conversation)).await?;
    Ok(())
}

/// `run-now`: fire immediately without moving the schedule. `None` when the schedule is busy
/// (its latest run's turn is active; 409 at the route). Returns the run and its new thread.
pub async fn run_now(
    conn: &mut PgConnection,
    row: &ScheduleRow,
) -> Result<Option<(RunRow, Uuid)>, ApiError> {
    let agent = agents::get(&mut *conn, row.agent_id)
        .await?
        .ok_or_else(super::service::not_found)?;
    // (phase 2, RFE-006) run-now starts agent work: 423 `frozen` for a frozen agent.
    crate::domains::freeze::service::ensure_agent_active(&mut *conn, agent.id).await?;
    // (P5-SCHED-ONCE) Run now of a one-time schedule is a manual run in its own thread too; it
    // changes neither `run_at` nor `completed_at`.
    let Some(st) = start_run(conn, row, &agent, true).await? else {
        return Ok(None);
    };
    repo::record_manual(conn, row.id, st.conversation).await?;
    Ok(Some((st.run, st.conversation)))
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
            // (P5-SCHED-ONCE) A one-time schedule is already Done: its row is not written from
            // a finishing run (see `on_turn_finished`).
            if sched.cron.is_some() {
                repo::reset_failures(conn, sched.id, "gated_quiet").await?;
            }
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
/// (P5-SCHED-ONCE, D-421) A one-time schedule is never counted or disabled; the notice is still
/// posted.
async fn record_failure(
    conn: &mut PgConnection,
    sched: &ScheduleRow,
    turn: Uuid,
    what: &str,
) -> Result<(), ApiError> {
    if sched.cron.is_none() {
        return notify_origin(
            conn,
            sched,
            "schedule_run",
            &format!("Scheduled task \"{}\" failed ({what}).", sched.name),
            Some(turn),
        )
        .await;
    }
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
        // (P5-SCHED-ONCE, C-1093) A one-time run is Done since it fired: its row is not written
        // here (a run in the origin holds that conversation's lock, and the fire path takes the
        // schedule row before the conversation). A run that happened in the origin gets no
        // "finished" notice: the answer is already there.
        "done" if sched.cron.is_none() => {
            if Some(t.conversation_id) != sched.origin_conversation_id {
                notify_origin(
                    conn,
                    &sched,
                    "schedule_run",
                    &format!("Scheduled task \"{}\" finished.", sched.name),
                    Some(t.id),
                )
                .await?;
            }
        }
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
