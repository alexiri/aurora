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
    if s.len() <= max {
        return s;
    }
    let mut e = max;
    while !s.is_char_boundary(e) {
        e -= 1;
    }
    &s[..e]
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
            | ReportBody::TurnProgress(_)
            | ReportBody::TurnResult(_)
            | ReportBody::SteerResult(_)
            | ReportBody::SendNowResult(_)
            | ReportBody::Session(_)
            | ReportBody::SessionPurged(_)
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
        ReportBody::SteerResult(_) | ReportBody::SendNowResult(_) => {
            // Unjoined follow-ups are promoted when the live turn ends (finish_turn); a missed
            // steer is never re-sent (`steer_delivered_at` is stamped).
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
        Some(_) => res(r, ReportStatus::Applied, None),
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
        "usage" => {
            if let Some(u) = d.get("usage").filter(|u| u.is_object()) {
                hosted::set_usage(tx, turn, u).await?;
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
    let msgs: Vec<Msg> = hosted::answer_buffer_take(tx, turn)
        .await?
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    for m in msgs.iter().filter(|m| !m.text.trim().is_empty()) {
        let mut p =
            json!({"turn_id": turn, "message_id": m.id, "text": clip(&m.text, TEXT_MAX_BYTES)});
        if let Some(rt) = &m.reply_to {
            p["reply_to"] = json!(rt);
        }
        conv::append_event(tx, cid, Some(turn), "text", &p).await?;
    }
    let status = tr.status.as_str();
    let code = tr.error_code.as_ref().map(|c| c.as_str().to_owned());
    let message = tr.error_message.as_deref().map(|m| clip(m, 2000));
    if status == "failed" {
        let code = code.as_deref().unwrap_or("harness_failed");
        let p = json!({"turn_id": turn, "code": code, "message": message.unwrap_or("")});
        conv::append_event(tx, cid, Some(turn), "error", &p).await?;
    }
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
        tr.usage.as_ref().filter(|u| u.is_object()),
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
    let Some(cid) = hosted::fire_watch(tx, wid, facts.agent_id).await? else {
        return Ok(res(r, ReportStatus::Duplicate, None));
    };
    let Some(c) = convs::get(&mut *tx, cid).await? else {
        return Ok(res(r, ReportStatus::Invalid, Some("unknown_conversation")));
    };
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
        },
    )
    .await?;
    let (Created::Turn(t) | Created::Followup(t)) = created;
    hosted::set_watch_turn(tx, wid, t.id).await?;
    Ok(res(r, ReportStatus::Applied, None))
}
