//! Supervisor/turn report handlers (docs/16 §W5 "Reports"). Every handler is idempotent (I-25):
//! turn transitions are fenced on the current status/boot, event batches on `turns.last_event_n`,
//! and channel reports on `(boot_id, channel_replica_id)`.

use super::lifecycle;
use crate::app::AppState;
use crate::domains::conversations::service::{self as conv, NewMessage};
use aurora_core::{ApiError, limits, sanitize};
use aurora_db::repo::conversations as convs;
use aurora_db::repo::hosted::{self, ProcessUpsert, SandboxFacts};
use aurora_db::repo::turns::{self, Created};
use aurora_protocol::runner::{
    HarnessEventWire, NumberedEvent, Report, ReportBody, ReportResult, ReportStatus,
};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

/// Per-payload text bound for durable events (payload cap is 64 KiB).
const TEXT_MAX_BYTES: usize = 48 * 1024;
/// Per-turn bound of buffered answer text.
const ANSWER_MAX_BYTES: usize = 1024 * 1024;

fn res(r: &Report, s: ReportStatus, detail: Option<&str>) -> ReportResult {
    ReportResult {
        report_id: r.report_id,
        status: s,
        detail: detail.map(str::to_owned),
    }
}

fn clip(s: &str, max: usize) -> &str {
    &s[..s.floor_char_boundary(max)]
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Msg {
    id: String,
    text: String,
    reply_to: Option<String>,
}

// The answer-rule buffer (text after the turn's last tool call, 10) is persisted in
// `turn_answer_buffers` under the turn lock (FUP-M4.1): a replica restart loses nothing.

async fn load_answers(tx: &mut PgConnection, turn: Uuid) -> Result<Vec<Msg>, ApiError> {
    Ok(hosted::answer_buffer_get(tx, turn)
        .await?
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default())
}

fn str_of<'a>(m: &'a serde_json::Map<String, Value>, k: &str) -> Option<&'a str> {
    m.get(k).and_then(Value::as_str)
}

/// Entry point for report kinds owned by this module; `None` = not handled here.
pub async fn apply(
    s: &AppState,
    tx: &mut PgConnection,
    replica: Uuid,
    facts: &SandboxFacts,
    r: &Report,
) -> Result<Option<ReportResult>, ApiError> {
    let handled = matches!(
        r.body,
        ReportBody::SupervisorReady(_)
            | ReportBody::TurnStarted(_)
            | ReportBody::TurnEvents(_)
            | ReportBody::TurnIdleEvents(_)
            | ReportBody::TurnProgress(_)
            | ReportBody::TurnResult(_)
            | ReportBody::SteerResult(_)
            | ReportBody::SendNowResult(_)
            | ReportBody::Session(_)
            | ReportBody::SessionPurged(_)
            | ReportBody::SessionReleased(_)
            | ReportBody::ProcessState(_)
            | ReportBody::WatchFired(_)
            | ReportBody::GateResult(_)
    );
    if !handled {
        return Ok(None);
    }
    // Channel reports are fenced on the current boot and the channel-holding replica.
    if facts.boot_id != Some(r.boot_id) || facts.channel_replica_id != Some(replica) {
        return Ok(Some(res(
            r,
            ReportStatus::Fenced,
            Some("not_channel_holder"),
        )));
    }
    let out = match &r.body {
        ReportBody::SupervisorReady(sr) => {
            let h = json!({
                "id": sr.harness.id.as_str(),
                "version": sr.version,
                "capabilities": sr.harness.capabilities.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
            });
            hosted::record_harness(tx, r.sandbox_id, r.boot_id, replica, &h).await?;
            conv::notify_work(tx).await?;
            res(r, ReportStatus::Applied, None)
        }
        ReportBody::TurnStarted(t) => started(tx, facts, r, t.turn_id).await?,
        ReportBody::TurnEvents(e) => events(s, tx, facts, r, e.turn_id, &e.events).await?,
        ReportBody::TurnIdleEvents(e) => idle_events(s, tx, facts, r, e.turn_id, &e.events).await?,
        ReportBody::TurnProgress(t) => {
            let own = own_turn(tx, facts, r, t.turn_id).await?;
            match own {
                Err(x) => x,
                Ok(_) => {
                    let ok = hosted::touch_progress(tx, t.turn_id).await?;
                    res(
                        r,
                        if ok {
                            ReportStatus::Applied
                        } else {
                            ReportStatus::Duplicate
                        },
                        None,
                    )
                }
            }
        }
        ReportBody::TurnResult(t) => result(s, tx, facts, r, t).await?,
        ReportBody::SteerResult(sr) => {
            // (FIX-225, C-1153, D-470) `delivered` needs nothing (the `message.joined` event sets
            // `joined`). A steer the harness missed or refused is never re-sent
            // (`steer_delivered_at` stays stamped): the follow-up stays `joining`, runs as the next
            // turn (finish_turn promotes it), and the person is told on its row.
            let reason = sr.outcome.as_str();
            if matches!(reason, "missed" | "refused") {
                let f = turns::get(&mut *tx, sr.followup_id)
                    .await?
                    .filter(|f| f.role == "followup" && f.agent_id == facts.agent_id);
                let Some(f) = f else {
                    return Ok(Some(res(
                        r,
                        ReportStatus::Invalid,
                        Some("unknown_followup"),
                    )));
                };
                convs::lock(&mut *tx, f.conversation_id).await?;
                if turns::set_steer_error(&mut *tx, f.id, reason)
                    .await?
                    .is_some()
                {
                    conv::append_event(
                        tx,
                        f.conversation_id,
                        f.parent_turn_id,
                        "followup.steer_failed",
                        &json!({"followup_id": f.id, "reason": reason}),
                    )
                    .await?;
                    // A chat thread hears it too (platform voice, no model text).
                    crate::domains::channels::mirror::post_platform_note(
                        tx,
                        f.conversation_id,
                        &format!("steer-failed:{}", f.id),
                        "Couldn't steer your message in (the agent missed it). It will run after the current turn.",
                    )
                    .await;
                }
            }
            res(r, ReportStatus::Applied, None)
        }
        ReportBody::SendNowResult(sn) => {
            // (FIX-225, C-1156, D-470) `moved` keeps `message.sent_now` (from the turn's events); an
            // answer that moved nothing is shown to the person.
            let outcome = sn.outcome.as_str();
            if matches!(outcome, "nothing_running" | "unsupported") {
                let t = match own_turn(tx, facts, r, sn.turn_id).await? {
                    Ok(t) => t,
                    Err(x) => return Ok(Some(x)),
                };
                convs::lock(&mut *tx, t.conversation_id).await?;
                conv::append_event(
                    tx,
                    t.conversation_id,
                    Some(t.id),
                    "turn.send_now_failed",
                    &json!({"turn_id": t.id, "outcome": outcome}),
                )
                .await?;
                crate::domains::channels::mirror::post_platform_note(
                    tx,
                    t.conversation_id,
                    &format!("send-now-failed:{}:{outcome}", t.id),
                    if outcome == "nothing_running" {
                        "Nothing was running to move to the background."
                    } else {
                        "This agent can't move a running step to the background."
                    },
                )
                .await;
            }
            res(r, ReportStatus::Applied, None)
        }
        ReportBody::Session(sr) => {
            let ok = hosted::set_session(
                tx,
                sr.conversation_id,
                facts.agent_id,
                &sr.session_ref,
                i64::try_from(sr.epoch).unwrap_or(i64::MAX),
            )
            .await?;
            res(
                r,
                if ok {
                    ReportStatus::Applied
                } else {
                    ReportStatus::Duplicate
                },
                None,
            )
        }
        ReportBody::SessionPurged(sp) => {
            let done = aurora_db::repo::lifecycle::complete_purge(
                tx,
                facts.agent_id,
                sp.conversation_id,
                &sp.session_ref,
                sp.ok,
            )
            .await?;
            res(
                r,
                if done {
                    ReportStatus::Applied
                } else {
                    ReportStatus::Duplicate
                },
                None,
            )
        }
        ReportBody::SessionReleased(sr) => {
            let done =
                complete_background_stop(tx, facts.agent_id, sr.conversation_id, sr.ok).await?;
            res(
                r,
                if done {
                    ReportStatus::Applied
                } else {
                    ReportStatus::Duplicate
                },
                None,
            )
        }
        ReportBody::ProcessState(ps) => {
            for p in &ps.processes {
                let Ok(id) = Uuid::parse_str(&p.process_id) else {
                    return Ok(Some(res(r, ReportStatus::Invalid, Some("bad_process_id"))));
                };
                let command = p.command.as_deref().unwrap_or("");
                hosted::upsert_process(
                    tx,
                    &ProcessUpsert {
                        id,
                        agent_id: facts.agent_id,
                        sandbox_id: r.sandbox_id,
                        boot_id: r.boot_id,
                        name: clip(command, 120),
                        command,
                        status: p.state.as_str(),
                        busy: p.busy.unwrap_or(false),
                        exit_code: p.exit_code,
                        tail: p.tail.as_deref().map(|t| clip(t, limits::PROCESS_TAIL)),
                    },
                )
                .await?;
            }
            res(r, ReportStatus::Applied, None)
        }
        ReportBody::WatchFired(w) => watch_fired(tx, facts, r, &w.watch_id, &w.detail).await?,
        ReportBody::GateResult(g) => {
            use crate::domains::schedules::fire::{GateApplied, gate_result};
            match gate_result(
                tx,
                facts.agent_id,
                g.turn_id,
                &g.outcome,
                g.output.as_deref(),
            )
            .await?
            {
                GateApplied::Applied => res(r, ReportStatus::Applied, None),
                GateApplied::Duplicate => res(r, ReportStatus::Duplicate, None),
                GateApplied::Invalid(d) => res(r, ReportStatus::Invalid, Some(d)),
            }
        }
        _ => res(r, ReportStatus::Invalid, Some("not_implemented")),
    };
    Ok(Some(out))
}

/// The turn exists and belongs to the sandbox's agent; `Err(result)` otherwise.
async fn own_turn(
    tx: &mut PgConnection,
    facts: &SandboxFacts,
    r: &Report,
    turn: Uuid,
) -> Result<Result<turns::TurnRow, ReportResult>, ApiError> {
    Ok(
        match turns::get(&mut *tx, turn)
            .await?
            .filter(|t| t.role == "turn")
        {
            Some(t) if t.agent_id == facts.agent_id => Ok(t),
            _ => Err(res(r, ReportStatus::Invalid, Some("unknown_turn"))),
        },
    )
}

async fn started(
    tx: &mut PgConnection,
    facts: &SandboxFacts,
    r: &Report,
    turn: Uuid,
) -> Result<ReportResult, ApiError> {
    let t = match own_turn(tx, facts, r, turn).await? {
        Ok(t) => t,
        Err(x) => return Ok(x),
    };
    let tb = hosted::turn_boot(tx, t.id).await?;
    if tb.is_some() && tb != Some(r.boot_id) {
        return Ok(res(r, ReportStatus::Fenced, Some("other_boot")));
    }
    Ok(match conv::start_turn(tx, turn, Some(r.boot_id)).await? {
        Some(row) => {
            // (FIX-158, D-319) The platform shows the agent is working while the turn runs.
            crate::domains::channels::mirror::on_turn_started(tx, &row).await?;
            res(r, ReportStatus::Applied, None)
        }
        None => res(r, ReportStatus::Duplicate, None),
    })
}

async fn events(
    s: &AppState,
    tx: &mut PgConnection,
    facts: &SandboxFacts,
    r: &Report,
    turn: Uuid,
    evs: &[NumberedEvent],
) -> Result<ReportResult, ApiError> {
    let t = match own_turn(tx, facts, r, turn).await? {
        Ok(t) => t,
        Err(x) => return Ok(x),
    };
    let cid = t.conversation_id;
    convs::lock(tx, cid).await?;
    let Some((_, _, status, last_n)) = hosted::lock_turn_events(tx, turn).await? else {
        return Ok(res(r, ReportStatus::Invalid, Some("unknown_turn")));
    };
    if !matches!(status.as_str(), "dispatched" | "running") {
        return Ok(res(r, ReportStatus::Duplicate, None));
    }
    let mut sorted: Vec<&NumberedEvent> = evs.iter().collect();
    sorted.sort_by_key(|e| e.n);
    let mut n_done = i64::from(last_n);
    let mut applied = false;
    // Buffer changes are staged and persisted in this transaction after the batch succeeded.
    let mut staged: Vec<(Uuid, StageOp)> = Vec::new();
    for e in sorted {
        let n = i64::try_from(e.n).unwrap_or(i64::MAX);
        if n <= n_done {
            continue;
        }
        n_done = n;
        applied = true;
        one_event(s, tx, cid, turn, &e.event, &mut staged).await?;
    }
    if applied {
        hosted::set_last_event_n(tx, turn, i32::try_from(n_done).unwrap_or(i32::MAX)).await?;
        // (P4-NARR) The turn's narration card follows its tool steps; a failure never fails the
        // report.
        {
            use aurora_db::DbError;
            use sqlx::Connection;
            let mut sp = tx.begin().await.map_err(DbError::from)?;
            match crate::domains::channels::narration::refresh(&mut sp, &t).await {
                Ok(()) => sp.commit().await.map_err(DbError::from)?,
                Err(e) => {
                    tracing::warn!(turn = %turn, error = %e.message(), "narration update not queued");
                    sp.rollback().await.map_err(DbError::from)?;
                }
            }
        }
        if !staged.is_empty() {
            let mut buf = load_answers(tx, turn).await?;
            for (_, op) in staged {
                op.apply(&mut buf);
            }
            let v = serde_json::to_value(&buf).unwrap_or_else(|_| json!([]));
            hosted::answer_buffer_set(tx, turn, &v).await?;
        }
    }
    Ok(res(
        r,
        if applied {
            ReportStatus::Applied
        } else {
            ReportStatus::Duplicate
        },
        None,
    ))
}

enum StageOp {
    Append(String, String, Option<String>),
    Clear,
    Retract(String),
    Replace(String, String),
}

/// (FIX-253, part B) The most `turn.idle.events` entries one report may add: the reader is
/// bounded (`12`), and this keeps a burst from flooding the transcript.
const AFTER_TURN_MAX: usize = 100;
/// What the conversation says when recording stops: at a cap, or with a task's budget used up.
const CAPPED: &str = "More activity was not recorded.";
const OUT_OF_BUDGET: &str = "More background activity happened but was not recorded: this task \
                             has used its message budget.";
/// The most after-the-turn entries one anchor turn keeps, and one conversation in all.
const AFTER_TURN_PER_ANCHOR: i64 = 500;
const AFTER_TURN_PER_CONVERSATION: i64 = 2_000;
/// (FIX-253, part B) The most text one after-the-turn message keeps.
const AFTER_TURN_TEXT_MAX: usize = 2000;

/// (FIX-253, part B) What the idle harness child did after the anchor turn ended (`12 §The idle
/// reader`): the supervisor watched the child and reports its own runs' events. They become the
/// conversation's durable **after the turn** entries — text and tool calls, masked and capped
/// like a turn's — attributed to the anchor turn (the web folds them under "Worked after the
/// turn"). The anchor must be the reporting sandbox's own turn: finished, run on the current
/// boot, and the conversation's latest finished one (an idle child belongs to it; a turn still
/// running is not idle, its own path maps the events). The entries are bounded per anchor and per
/// conversation, and stop in a peer task's conversation whose message budget is used up. What the
/// idle runs spent is counted in the usage ledger in every one of these cases (a stale anchor
/// too): the tokens were spent. Recording stops; the idle work itself is not stopped.
async fn idle_events(
    s: &AppState,
    tx: &mut PgConnection,
    facts: &SandboxFacts,
    r: &Report,
    turn: Uuid,
    evs: &[NumberedEvent],
) -> Result<ReportResult, ApiError> {
    let t = match own_turn(tx, facts, r, turn).await? {
        Ok(t) => t,
        Err(x) => return Ok(x),
    };
    if matches!(t.status.as_str(), "dispatched" | "running") {
        return Ok(res(r, ReportStatus::Invalid, Some("turn_still_running")));
    }
    if hosted::turn_boot(tx, t.id).await? != Some(r.boot_id) {
        return Ok(res(r, ReportStatus::Fenced, Some("other_boot")));
    }
    let cid = t.conversation_id;
    convs::lock(tx, cid).await?;
    let mut sorted: Vec<&NumberedEvent> = evs.iter().collect();
    sorted.sort_by_key(|e| e.n);
    let (spent, sorted): (Vec<&NumberedEvent>, Vec<&NumberedEvent>) = sorted
        .into_iter()
        .partition(|e| e.event.kind.as_str() == "usage");
    for e in spent {
        let Some(claim) = e.event.data.get("usage").filter(|u| u.is_object()) else {
            continue;
        };
        // The same trust rule as a turn's usage.
        let usage = super::model::usage_for_turn(s, tx, turn, claim).await?;
        let n = i64::try_from(e.n).unwrap_or(i64::MAX);
        aurora_db::repo::usage::record_idle(tx, turn, n, &usage).await?;
    }
    if turns::latest_finished(&mut *tx, cid).await? != Some(turn) {
        return Ok(res(r, ReportStatus::Invalid, Some("stale_anchor")));
    }
    let out_of_budget = aurora_db::repo::peer_tasks::by_task_conversation(&mut *tx, cid)
        .await?
        .is_some_and(|task| task.messages_used >= task.message_budget);
    let (mut of_anchor, mut of_conversation) = after_turn_counts(tx, cid, turn).await?;
    for e in sorted.into_iter().take(AFTER_TURN_MAX) {
        let d = &e.event.data;
        let (ty, mut p) = match e.event.kind.as_str() {
            "tool.started" => (
                "after_turn.tool",
                json!({"turn_id": turn,
                    "call_id": clip(str_of(d, "call_id").unwrap_or(""), 128),
                    "name": clip(str_of(d, "name").unwrap_or(""), 128),
                    "summary": sanitize::one_line(
                        &sanitize::reason(str_of(d, "summary").unwrap_or(""), 200), 200)}),
            ),
            "tool.finished" => {
                let mut p = json!({"turn_id": turn,
                    "call_id": clip(str_of(d, "call_id").unwrap_or(""), 128),
                    "ok": d.get("ok").and_then(Value::as_bool).unwrap_or(false)});
                if let Some(sm) = str_of(d, "summary") {
                    p["summary"] = json!(sanitize::one_line(&sanitize::reason(sm, 200), 200));
                }
                ("after_turn.tool_end", p)
            }
            "text.delta" => {
                // Masked like a start-failure reason (tokens, `Authorization:`, `KEY=`), and
                // recorded once per message: the idle run's text arrives whole.
                let text = sanitize::reason(str_of(d, "delta").unwrap_or(""), AFTER_TURN_TEXT_MAX);
                if text.is_empty() {
                    continue;
                }
                let mid = clip(str_of(d, "message_id").unwrap_or(""), 128);
                (
                    "after_turn.text",
                    json!({"turn_id": turn, "message_id": mid, "text": text}),
                )
            }
            _ => continue, // the reader forwards only text and tool calls
        };
        let key = if ty == "after_turn.text" {
            "message_id"
        } else {
            "call_id"
        };
        // An event without an id of its own is known by its place in the anchor's watch.
        if p[key].as_str().is_none_or(str::is_empty) {
            p[key] = json!(format!("{turn}:{}", e.n));
        }
        if already_recorded(tx, cid, ty, p[key].as_str().unwrap_or("")).await? {
            continue;
        }
        if out_of_budget {
            note_not_recorded(tx, cid, turn, "budget", OUT_OF_BUDGET).await?;
            break;
        }
        if of_conversation >= AFTER_TURN_PER_CONVERSATION {
            note_not_recorded(tx, cid, turn, "conversation", CAPPED).await?;
            break;
        }
        if of_anchor >= AFTER_TURN_PER_ANCHOR {
            note_not_recorded(tx, cid, turn, "turn", CAPPED).await?;
            break;
        }
        conv::append_event(tx, cid, Some(turn), ty, &p).await?;
        of_anchor += 1;
        of_conversation += 1;
    }
    Ok(res(r, ReportStatus::Applied, None))
}

/// How many after-the-turn entries exist: `(of the anchor turn, of the conversation)`.
async fn after_turn_counts(
    tx: &mut PgConnection,
    cid: Uuid,
    turn: Uuid,
) -> Result<(i64, i64), ApiError> {
    Ok(sqlx::query_as(
        "SELECT count(*) FILTER (WHERE turn_id = $2), count(*) FROM transcript_events
         WHERE conversation_id = $1 AND starts_with(type, 'after_turn.')",
    )
    .bind(cid)
    .bind(turn)
    .fetch_one(&mut *tx)
    .await
    .map_err(aurora_db::DbError::from)?)
}

/// One line says that further activity is not recorded: once per anchor turn for its cap, once
/// per conversation for the conversation's cap and for a used-up task budget.
async fn note_not_recorded(
    tx: &mut PgConnection,
    cid: Uuid,
    turn: Uuid,
    scope: &str,
    text: &str,
) -> Result<(), ApiError> {
    let said: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM transcript_events
          WHERE conversation_id = $1 AND type = 'notice'
            AND payload->>'code' = 'after_turn_capped' AND payload->>'scope' = $2
            AND ($2 <> 'turn' OR payload->>'turn_id' = $3))",
    )
    .bind(cid)
    .bind(scope)
    .bind(turn.to_string())
    .fetch_one(&mut *tx)
    .await
    .map_err(aurora_db::DbError::from)?;
    if !said {
        conv::append_event(
            tx,
            cid,
            Some(turn),
            "notice",
            &json!({"code": "after_turn_capped", "turn_id": turn, "scope": scope, "text": text}),
        )
        .await?;
    }
    Ok(())
}

/// (FIX-253, part B) Whether this after-the-turn item is already on the transcript (a
/// redelivered report must not duplicate the idle child's work).
async fn already_recorded(
    tx: &mut PgConnection,
    cid: Uuid,
    ty: &str,
    key: &str,
) -> Result<bool, ApiError> {
    let key_col = if ty == "after_turn.text" {
        "message_id"
    } else {
        "call_id"
    };
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM transcript_events
         WHERE conversation_id = $1 AND type = $2 AND payload->>$3 = $4",
    )
    .bind(cid)
    .bind(ty)
    .bind(key_col)
    .bind(key)
    .fetch_one(&mut *tx)
    .await
    .map_err(aurora_db::DbError::from)?
        > 0)
}

impl StageOp {
    fn apply(self, v: &mut Vec<Msg>) {
        match self {
            Self::Clear => v.clear(),
            Self::Retract(id) => v.retain(|x| x.id != id),
            Self::Replace(id, text) => {
                if let Some(x) = v.iter_mut().find(|x| x.id == id) {
                    x.text = text;
                }
            }
            Self::Append(id, delta, reply_to) => {
                let total: usize = v.iter().map(|x| x.text.len()).sum();
                if total + delta.len() > ANSWER_MAX_BYTES {
                    return;
                }
                match v.iter_mut().find(|x| x.id == id) {
                    Some(x) => x.text.push_str(&delta),
                    None => v.push(Msg {
                        id,
                        text: delta,
                        reply_to,
                    }),
                }
            }
        }
    }
}

async fn one_event(
    s: &AppState,
    tx: &mut PgConnection,
    cid: Uuid,
    turn: Uuid,
    ev: &HarnessEventWire,
    staged: &mut Vec<(Uuid, StageOp)>,
) -> Result<(), ApiError> {
    let d = &ev.data;
    match ev.kind.as_str() {
        "text.delta" => {
            let delta = str_of(d, "delta").unwrap_or("");
            let mid = str_of(d, "message_id").unwrap_or("m");
            let rt = str_of(d, "reply_to");
            s.conversation_bus
                .publish_delta(&s.pool, cid, turn, "text.delta", (Some(mid), rt), delta)
                .await;
            staged.push((
                turn,
                StageOp::Append(mid.to_owned(), delta.to_owned(), rt.map(str::to_owned)),
            ));
        }
        "thinking.delta" => {
            let delta = str_of(d, "delta").unwrap_or("");
            s.conversation_bus
                .publish_delta(&s.pool, cid, turn, "thinking.delta", (None, None), delta)
                .await;
        }
        "activity" => {
            let line = sanitize::one_line(str_of(d, "line").unwrap_or(""), 120);
            s.conversation_bus
                .publish_delta(&s.pool, cid, turn, "activity", (None, None), &line)
                .await;
        }
        "tool.started" => {
            // A tool call is a structural break: narration before it stays ephemeral.
            staged.push((turn, StageOp::Clear));
            let p = json!({"turn_id": turn,
                "call_id": clip(str_of(d, "call_id").unwrap_or(""), 128),
                "name": clip(str_of(d, "name").unwrap_or(""), 128),
                "summary": sanitize::one_line(str_of(d, "summary").unwrap_or(""), 200)});
            conv::append_event(tx, cid, Some(turn), "tool.started", &p).await?;
        }
        "tool.finished" => {
            let mut p = json!({"turn_id": turn,
                "call_id": clip(str_of(d, "call_id").unwrap_or(""), 128),
                "ok": d.get("ok").and_then(Value::as_bool).unwrap_or(false)});
            if let Some(sm) = str_of(d, "summary") {
                p["summary"] = json!(sanitize::one_line(sm, 200));
            }
            conv::append_event(tx, cid, Some(turn), "tool.finished", &p).await?;
        }
        "correction" => {
            let mid = str_of(d, "message_id").unwrap_or("");
            let action = str_of(d, "action").unwrap_or("");
            if !matches!(action, "retract" | "replace") {
                return Ok(());
            }
            let mut p = json!({"turn_id": turn, "message_id": mid, "action": action});
            if let Some(t) = str_of(d, "text") {
                p["text"] = json!(clip(t, TEXT_MAX_BYTES));
            }
            conv::append_event(tx, cid, Some(turn), "correction", &p).await?;
            staged.push((
                turn,
                match (action, str_of(d, "text")) {
                    ("replace", Some(t)) => StageOp::Replace(mid.to_owned(), t.to_owned()),
                    _ => StageOp::Retract(mid.to_owned()),
                },
            ));
        }
        "message.joined" => {
            let Some(fid) = str_of(d, "followup_id").and_then(|x| Uuid::parse_str(x).ok()) else {
                return Ok(());
            };
            let mine = turns::get(&mut *tx, fid)
                .await?
                .is_some_and(|f| f.role == "followup" && f.parent_turn_id == Some(turn));
            if mine && turns::mark_joined(tx, fid).await?.is_some() {
                let p = json!({"turn_id": turn, "followup_id": fid});
                conv::append_event(tx, cid, Some(turn), "message.joined", &p).await?;
            }
        }
        "message.sent_now" => {
            conv::append_event(
                tx,
                cid,
                Some(turn),
                "message.sent_now",
                &json!({"turn_id": turn}),
            )
            .await?;
        }
        // (FIX-253, D-490) The harness child's own background work, listed by the supervisor at
        // turn end: one visible **Background activity** entry in the conversation while nothing
        // else stands, so the reader can see what keeps running and stop it.
        "background" => {
            let Some(items) = d.get("items").and_then(Value::as_array) else {
                return Ok(());
            };
            let items: Vec<Value> = items
                .iter()
                .filter_map(|i| {
                    // (FIX-253 review) The command is displayed text: masked like a start-failure
                    // reason (tokens, `Authorization:`, `KEY=value` env assignments), capped.
                    let command = sanitize::reason(i.get("command")?.as_str()?, 200);
                    (!command.is_empty()).then(|| {
                        json!({"command": command,
                            "started_at": i.get("started_at").and_then(Value::as_u64).unwrap_or(0)})
                    })
                })
                .take(10)
                .collect();
            if items.is_empty() {
                return Ok(());
            }
            let live = sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM transcript_events e
                 WHERE e.conversation_id = $1 AND e.type = 'notice'
                   AND e.payload->>'code' = 'background_activity'
                   AND NOT EXISTS (
                     SELECT 1 FROM transcript_events e2
                     WHERE e2.conversation_id = e.conversation_id AND e2.seq > e.seq
                       AND e2.type = 'notice' AND e2.payload->>'code' = 'background_stopped')",
            )
            .bind(cid)
            .fetch_one(&mut *tx)
            .await
            .map_err(aurora_db::DbError::from)?;
            if live > 0 {
                return Ok(());
            }
            let list: Vec<String> = items
                .iter()
                .map(|i| format!("`{}`", i["command"].as_str().unwrap_or_default()))
                .collect();
            conv::append_event(
                tx,
                cid,
                Some(turn),
                "notice",
                &json!({
                    "code": "background_activity",
                    "turn_id": turn,
                    "items": items,
                    "text": format!("Background activity, still running: {}", list.join(", ")),
                }),
            )
            .await?;
        }
        "usage" => {
            if let Some(u) = d.get("usage").filter(|u| u.is_object()) {
                // (P5-USAGE-1, C-1001, D-359(d)) Reduced by the trust rule when it is stored, so a turn
                // that ends without a terminal usage (abort, dead dispatch) still holds a value the
                // ledger may count.
                let reduced = super::model::usage_for_turn(s, tx, turn, u).await?;
                hosted::set_usage(tx, turn, &reduced).await?;
            }
        }
        _ => {} // unknown kinds are ignored
    }
    Ok(())
}

async fn result(
    s: &AppState,
    tx: &mut PgConnection,
    facts: &SandboxFacts,
    r: &Report,
    tr: &aurora_protocol::runner::TurnResult,
) -> Result<ReportResult, ApiError> {
    let turn = tr.turn_id;
    let t = match own_turn(tx, facts, r, turn).await? {
        Ok(t) => t,
        Err(x) => return Ok(x),
    };
    let cid = t.conversation_id;
    convs::lock(tx, cid).await?;
    let live = turns::get(&mut *tx, turn)
        .await?
        .is_some_and(|t| matches!(t.status.as_str(), "queued" | "dispatched" | "running"));
    if !live {
        return Ok(res(r, ReportStatus::Duplicate, None));
    }
    s.conversation_bus.flush_turn(&s.pool, cid, turn).await;
    let status = tr.status.as_str();
    let code = tr.error_code.as_ref().map(|c| c.as_str().to_owned());
    let message = tr.error_message.as_deref().map(|m| clip(m, 2000));
    // The answer rule: the text buffered since the last tool call becomes durable `text` events
    // (also on every API-side failure path, `lifecycle::fail`; FIX-115). (P5-REFINE-7) A stopped
    // turn's text is `partial` (C-1075); a failed turn's harness error line becomes the error's
    // `detail` instead of an answer (C-1076).
    let dropped = lifecycle::drain_answer(
        tx,
        cid,
        turn,
        lifecycle::Ending::of(status, Some(message.unwrap_or(""))),
    )
    .await?;
    if status == "failed" {
        let code = code.as_deref().unwrap_or("harness_failed");
        let p = lifecycle::error_payload(turn, code, message.unwrap_or(""), dropped.as_deref());
        conv::append_event(tx, cid, Some(turn), "error", &p).await?;
    }
    // (P5-MODELS-2, D-304) The harness's claim, reduced by the trust rule and stamped from the turn's basis.
    let usage = match tr.usage.as_ref().filter(|u| u.is_object()) {
        Some(claim) => Some(super::model::usage_for_turn(s, tx, turn, claim).await?),
        None => None,
    };
    let done = lifecycle::finish(
        tx,
        turn,
        status,
        if status == "failed" {
            Some(code.as_deref().unwrap_or("harness_failed"))
        } else {
            code.as_deref()
        },
        message,
        usage.as_ref(),
    )
    .await?;
    Ok(match done {
        Some(_) => {
            conv::notify_work(tx).await?;
            res(r, ReportStatus::Applied, None)
        }
        None => res(r, ReportStatus::Duplicate, None),
    })
}

/// (FIX-253 review) `session.released`: an acknowledged release writes the durable **Background
/// activity stopped** notice and closes the request (the abort flag and the in-flight mark are
/// cleared); a deferral (`ok = false`, a turn was running) keeps the request pending — it is
/// re-sent after its lease, and the notice is only written once the release really happened.
/// (FIX-253 fixes2 B) A release that reports failure writes a durable **stopping did not work**
/// notice (once per turn) while keeping the abort flag set, so the UI re-enables Stop and the
/// dispatch lease retries the release.
async fn complete_background_stop(
    tx: &mut PgConnection,
    agent: Uuid,
    conversation_id: Uuid,
    ok: bool,
) -> Result<bool, ApiError> {
    if !ok {
        let Some((turn_id,)) = sqlx::query_as::<_, (Uuid,)>(
            "SELECT t.id FROM turns t
              WHERE t.agent_id = $1 AND t.conversation_id = $2 AND t.role = 'turn'
                AND t.abort_requested_at IS NOT NULL
              ORDER BY t.created_at LIMIT 1
              FOR UPDATE OF t",
        )
        .bind(agent)
        .bind(conversation_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(aurora_db::DbError::from)?
        else {
            return Ok(false);
        };
        let told = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM transcript_events
              WHERE conversation_id = $1 AND turn_id = $2 AND type = 'notice'
                AND payload->>'code' = 'background_stop_failed'",
        )
        .bind(conversation_id)
        .bind(turn_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(aurora_db::DbError::from)?;
        if told == 0 {
            conv::append_event(
                tx,
                conversation_id,
                Some(turn_id),
                "notice",
                &json!({"code": "background_stop_failed", "turn_id": turn_id,
                    "text": "Stopping did not work: try again."}),
            )
            .await?;
        }
        return Ok(false);
    }
    let Some((turn_id,)) = sqlx::query_as::<_, (Uuid,)>(
        "SELECT t.id FROM turns t
          WHERE t.agent_id = $1 AND t.conversation_id = $2 AND t.role = 'turn'
            AND t.status NOT IN ('dispatched', 'running')
            AND t.abort_requested_at IS NOT NULL
            AND EXISTS (SELECT 1 FROM transcript_events e
              WHERE e.conversation_id = t.conversation_id AND e.turn_id = t.id
                AND e.type = 'notice' AND e.payload->>'code' = 'background_activity'
                AND NOT EXISTS (SELECT 1 FROM transcript_events e2
                  WHERE e2.conversation_id = e.conversation_id AND e2.seq > e.seq
                    AND e2.type = 'notice' AND e2.payload->>'code' = 'background_stopped'))
          ORDER BY t.created_at LIMIT 1
          FOR UPDATE OF t",
    )
    .bind(agent)
    .bind(conversation_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(aurora_db::DbError::from)?
    else {
        return Ok(false);
    };
    sqlx::query(
        "UPDATE turns SET abort_requested_at = NULL, steer_delivered_at = NULL, updated_at = now()
         WHERE id = $1",
    )
    .bind(turn_id)
    .execute(&mut *tx)
    .await
    .map_err(aurora_db::DbError::from)?;
    conv::append_event(
        tx,
        conversation_id,
        Some(turn_id),
        "notice",
        &json!({"code": "background_stopped", "turn_id": turn_id,
            "text": "Background activity stopped."}),
    )
    .await?;
    Ok(true)
}

async fn watch_fired(
    tx: &mut PgConnection,
    facts: &SandboxFacts,
    r: &Report,
    watch_id: &str,
    detail: &str,
) -> Result<ReportResult, ApiError> {
    let Ok(wid) = Uuid::parse_str(watch_id) else {
        return Ok(res(r, ReportStatus::Invalid, Some("bad_watch_id")));
    };
    // (phase 2, RFE-006) Armed watches of a frozen agent are kept, never fired.
    if aurora_db::repo::freeze::is_agent_frozen(&mut *tx, facts.agent_id).await? {
        return Ok(res(r, ReportStatus::Invalid, Some("frozen")));
    }
    let Some(cid) = hosted::fire_watch(tx, wid, facts.agent_id).await? else {
        return Ok(res(r, ReportStatus::Duplicate, None));
    };
    let Some(c) = convs::get(&mut *tx, cid).await? else {
        return Ok(res(r, ReportStatus::Invalid, Some("unknown_conversation")));
    };
    // (FIX-253, D-490(b); security review, medium 2) The firing is a `waiting` task's wake —
    // through the one wake helper, before any turn starts: it re-checks the requester's rights
    // and the freeze, and a refused wake cancels the task with its close report (no turn).
    let wake = crate::domains::peer_tasks::service::resume_on_wake(tx, cid).await?;
    if wake.is_cancelled() {
        return Ok(res(r, ReportStatus::Applied, None));
    }
    let text = format!("A watch you set fired: {}", sanitize::block(detail, 4096));
    let created = conv::create_turn_tx(
        tx,
        &c,
        &NewMessage {
            door: json!({"kind": "watch"}),
            speaker_kind: "system",
            speaker_user_id: None,
            speaker_label: "Watch",
            text: &text,
            source_event_id: None,
            attachment_ids: &[],
            reply_to_seq: None,
        },
    )
    .await?;
    let (Created::Turn(t) | Created::Followup(t)) = created;
    hosted::set_watch_turn(tx, wid, t.id).await?;
    Ok(res(r, ReportStatus::Applied, None))
}
