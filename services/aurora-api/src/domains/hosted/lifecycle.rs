//! Turn lifecycle helpers shared by the due-work arms, report handlers and routes.

use crate::app::AppState;
use crate::domains::conversations::service as conv;
use aurora_core::{ApiError, limits};
use aurora_db::repo::credentials;
use aurora_db::repo::hosted;
use aurora_db::repo::turns::TurnRow;
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

/// Bound of one durable `text` event written from the answer buffer.
const TEXT_MAX_BYTES: usize = 48 * 1024;

/// Bound of the `detail` of an `error` event (C-1076), in characters.
const ERROR_DETAIL_MAX: usize = 2000;
/// How the Claude Code adapter prints a failed model call as an assistant message (C-1076).
const HARNESS_ERROR_PREFIX: &str = "API Error";

/// How the turn whose answer is drained ends (P5-REFINE-7).
#[derive(Clone, Copy)]
pub enum Ending<'a> {
    /// `done`, or an ending that keeps the text as it is.
    Done,
    /// (C-1075) Stopped: the text written so far is kept and marked `partial: true`.
    Aborted,
    /// (C-1076) Failed with this message: the harness's own error line is not the agent's answer.
    Failed(&'a str),
}

impl<'a> Ending<'a> {
    /// The ending a harness status reports.
    pub fn of(status: &str, message: Option<&'a str>) -> Ending<'a> {
        match status {
            "aborted" => Ending::Aborted,
            "failed" => Ending::Failed(message.unwrap_or("")),
            _ => Ending::Done,
        }
    }
}

/// (C-1076) Whether buffered text is the harness's error line rather than the agent's words: it
/// equals, contains or starts with the failure message, or starts with `API Error`.
pub fn is_harness_error_text(text: &str, message: &str) -> bool {
    let t = text.trim();
    let m = message.trim();
    t.starts_with(HARNESS_ERROR_PREFIX) || (!m.is_empty() && t.contains(m))
}

/// (C-1076) The `detail` of an `error` event: multi-line allowed, control characters removed,
/// at most 2,000 characters.
pub fn error_detail(text: &str) -> String {
    text.trim()
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .take(ERROR_DETAIL_MAX)
        .collect()
}

/// (FIX-115, `10` §The answer rule) Write the text buffered since the turn's last tool call as
/// durable `text` events and delete the buffer row, under the conversation lock — for every way
/// a turn can end, not only a harness result. Nothing is discarded silently: (P5-REFINE-7) a
/// stopped turn's text is marked `partial: true` (C-1075); a failed turn's harness error line is
/// not written as a `text` event but returned, for the `error` event's `detail` (C-1076).
pub async fn drain_answer(
    conn: &mut PgConnection,
    conversation: Uuid,
    turn: Uuid,
    ending: Ending<'_>,
) -> Result<Option<String>, ApiError> {
    aurora_db::repo::conversations::lock(&mut *conn, conversation).await?;
    let Some(Value::Array(msgs)) = hosted::answer_buffer_take(conn, turn).await? else {
        return Ok(None);
    };
    let mut dropped: Vec<&str> = Vec::new();
    for m in &msgs {
        let text = m.get("text").and_then(Value::as_str).unwrap_or("");
        if text.trim().is_empty() {
            continue;
        }
        if let Ending::Failed(message) = ending
            && is_harness_error_text(text, message)
        {
            dropped.push(text.trim());
            continue;
        }
        let mut end = text.len().min(TEXT_MAX_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let mut p = json!({"turn_id": turn, "message_id": m.get("id").cloned().unwrap_or(Value::Null),
            "text": &text[..end]});
        if let Some(rt) = m.get("reply_to").filter(|v| !v.is_null()) {
            p["reply_to"] = rt.clone();
        }
        if matches!(ending, Ending::Aborted) {
            p["partial"] = json!(true);
        }
        conv::append_event(conn, conversation, Some(turn), "text", &p).await?;
    }
    Ok((!dropped.is_empty()).then(|| dropped.join("\n")))
}

/// (C-1076) The `error` event payload: `{ turn_id, code, message, detail? }`; `detail` is the
/// harness's dropped line, else the message.
pub fn error_payload(turn: Uuid, code: &str, message: &str, dropped: Option<&str>) -> Value {
    let mut p = json!({"turn_id": turn, "code": code, "message": message});
    let detail = error_detail(dropped.unwrap_or(message));
    if !detail.is_empty() {
        p["detail"] = json!(detail);
    }
    p
}

/// End a turn (fenced): `turn.finished` + follow-up promotion (BE-051), then the turn's context
/// credential expires `CONTEXT_CREDENTIAL_GRACE` from now (08 §7). `None` on replay.
#[allow(clippy::too_many_arguments)]
pub async fn finish(
    conn: &mut PgConnection,
    turn: Uuid,
    status: &str,
    error_code: Option<&str>,
    error_message: Option<&str>,
    usage: Option<&Value>,
) -> Result<Option<TurnRow>, ApiError> {
    let Some(row) = conv::finish_turn(conn, turn, status, error_code, error_message, usage).await?
    else {
        return Ok(None);
    };
    let ids =
        hosted::expire_turn_credentials(conn, turn, limits::CONTEXT_CREDENTIAL_GRACE.as_secs_f64())
            .await?;
    for id in ids {
        credentials::notify_gateway_invalidate(&mut *conn, "credential", id).await?;
    }
    // (FIX-253, D-490) The self-check reads whether an open obligation existed at turn end,
    // before the hooks below close any of it.
    let obligation_open = open_obligation(conn, &row).await?;
    crate::domains::schedules::fire::on_turn_finished(conn, &row).await?;
    crate::domains::webhooks::on_turn_finished(conn, &row).await?;
    crate::domains::peer_tasks::service::on_turn_finished(conn, &row).await?;
    crate::domains::channels::mirror::on_turn_finished(conn, &row).await?;
    if obligation_open {
        no_wakeup_notice(conn, &row).await?;
    }
    Ok(Some(row))
}

/// (FIX-253, D-490) The self-check's signal, never word heuristics on the reply: this agent
/// turn ended `done` inside a delegated task's conversation whose peer task is still open —
/// the agent owes the task's report and nothing else will wake it for the task. (On the asking
/// side the task's own report follow-up is the wake, so it is not a notice case.)
#[allow(clippy::type_complexity)]
async fn open_obligation(conn: &mut PgConnection, t: &TurnRow) -> Result<bool, ApiError> {
    if t.status != "done" || t.speaker_kind != "agent" {
        return Ok(false);
    }
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM peer_tasks pt
            WHERE pt.task_conversation_id = $1 AND pt.status = 'open'
         )",
    )
    .bind(t.conversation_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(aurora_db::DbError::from)?)
}

/// (FIX-253, D-490) One visible notice after an agent turn that ended with an open obligation
/// and nothing set to wake the agent; the web offers a one-time reminder under it. Nothing is
/// recorded when a watch or a schedule is already set.
async fn no_wakeup_notice(conn: &mut PgConnection, t: &TurnRow) -> Result<(), ApiError> {
    let wake_registered = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM watches w
            WHERE w.conversation_id = $1 AND w.status = 'armed'
         ) OR EXISTS (
            SELECT 1 FROM schedules s
            WHERE s.origin_conversation_id = $1 AND s.enabled AND s.completed_at IS NULL
         )",
    )
    .bind(t.conversation_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(aurora_db::DbError::from)?;
    if wake_registered {
        return Ok(());
    }
    // (FIX-253, D-490(b)) A task the peer hook moved to `waiting` has a wake (the pending
    // approval's decision, if nothing else): no no-wakeup notice for it.
    let waiting = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM peer_tasks pt
            WHERE pt.task_conversation_id = $1 AND pt.status = 'waiting'
         )",
    )
    .bind(t.conversation_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(aurora_db::DbError::from)?;
    if waiting {
        return Ok(());
    }
    let name = aurora_db::repo::agents::get(&mut *conn, t.agent_id)
        .await?
        .map(|a| a.name)
        .unwrap_or_else(|| "The agent".to_owned());
    conv::append_event(
        conn,
        t.conversation_id,
        Some(t.id),
        "notice",
        &json!({
            "code": "no_wakeup",
            "text": format!("{name} said it would continue, but nothing is set to wake it."),
            "agent_id": t.agent_id,
            "agent_name": name,
            "turn_id": t.id,
        }),
    )
    .await?;
    Ok(())
}

/// Fail a turn with an `error` event followed by `turn.finished`.
pub async fn fail(
    conn: &mut PgConnection,
    turn: Uuid,
    conversation: Uuid,
    code: &str,
    message: &str,
) -> Result<Option<TurnRow>, ApiError> {
    fail_in(
        conn,
        turn,
        conversation,
        code,
        message,
        &["queued", "dispatched", "running"],
    )
    .await
}

/// (FIX-093) Fail a turn only while it is still `queued`, decided under the conversation lock: the
/// readiness sweep works from a snapshot, and a turn dispatch has since handed to a ready
/// sandbox must not be ended while its harness runs.
pub async fn fail_queued(
    conn: &mut PgConnection,
    turn: Uuid,
    conversation: Uuid,
    code: &str,
    message: &str,
) -> Result<Option<TurnRow>, ApiError> {
    aurora_db::repo::conversations::lock(&mut *conn, conversation).await?;
    fail_in(conn, turn, conversation, code, message, &["queued"]).await
}

async fn fail_in(
    conn: &mut PgConnection,
    turn: Uuid,
    conversation: Uuid,
    code: &str,
    message: &str,
    from: &[&str],
) -> Result<Option<TurnRow>, ApiError> {
    let Some(t) = aurora_db::repo::turns::get(&mut *conn, turn).await? else {
        return Ok(None);
    };
    if t.role != "turn" || !from.contains(&t.status.as_str()) {
        return Ok(None);
    }
    aurora_db::repo::conversations::lock(&mut *conn, conversation).await?;
    // (FIX-115) The answer text the agent had already written is kept, before the error;
    // (C-1076) the harness's own error line is the error's `detail`, not an answer.
    let dropped = drain_answer(conn, conversation, turn, Ending::Failed(message)).await?;
    conv::append_event(
        conn,
        conversation,
        Some(turn),
        "error",
        &error_payload(turn, code, message, dropped.as_deref()),
    )
    .await?;
    finish(conn, turn, "failed", Some(code), Some(message), None).await
}

/// Flush buffered deltas of a finished turn (call after commit).
pub async fn flush_deltas(state: &AppState, conversation: Uuid, turn: Uuid) {
    state
        .conversation_bus
        .flush_turn(&state.pool, conversation, turn)
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_harness_error_line_is_recognised_and_the_agents_words_are_not() {
        let m = "No connected db.";
        assert!(is_harness_error_text("API Error: 400 No connected db.", m));
        assert!(is_harness_error_text("API Error: 529 overloaded", ""));
        assert!(is_harness_error_text("  No connected db.  ", m));
        assert!(is_harness_error_text("No connected db. (request 42)", m));
        assert!(is_harness_error_text("Upstream said: No connected db.", m));
        assert!(!is_harness_error_text(
            "Here is the summary you asked for.",
            m
        ));
        // An empty message never matches everything.
        assert!(!is_harness_error_text("I looked at the logs.", ""));
        // The prefix is the adapter's, case-sensitive.
        assert!(!is_harness_error_text(
            "api error handling is described below",
            ""
        ));
    }

    #[test]
    fn the_detail_keeps_lines_drops_control_characters_and_is_bounded() {
        assert_eq!(error_detail(" a\nb\tc\u{7}\u{1b}d "), "a\nb\tcd");
        assert_eq!(error_detail(&"é".repeat(3000)).chars().count(), 2000);
        let p = error_payload(
            Uuid::nil(),
            "harness_failed",
            "boom",
            Some("API Error: 400 x"),
        );
        assert_eq!(p["message"], "boom");
        assert_eq!(p["detail"], "API Error: 400 x");
        let p = error_payload(Uuid::nil(), "harness_failed", "boom", None);
        assert_eq!(p["detail"], "boom");
        let p = error_payload(Uuid::nil(), "harness_failed", "", None);
        assert!(p.get("detail").is_none());
    }

    #[test]
    fn the_ending_follows_the_reported_status() {
        assert!(matches!(Ending::of("aborted", None), Ending::Aborted));
        assert!(matches!(
            Ending::of("failed", Some("x")),
            Ending::Failed("x")
        ));
        assert!(matches!(Ending::of("failed", None), Ending::Failed("")));
        assert!(matches!(Ending::of("done", None), Ending::Done));
    }
}
