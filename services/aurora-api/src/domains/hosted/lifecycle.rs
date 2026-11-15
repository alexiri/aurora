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

/// (FIX-115, `10` §The answer rule) Write the text buffered since the turn's last tool call as
/// durable `text` events and delete the buffer row, under the conversation lock — for every way
/// a turn can end, not only a harness result. Nothing is discarded silently.
pub async fn drain_answer(
    conn: &mut PgConnection,
    conversation: Uuid,
    turn: Uuid,
) -> Result<(), ApiError> {
    aurora_db::repo::conversations::lock(&mut *conn, conversation).await?;
    let Some(Value::Array(msgs)) = hosted::answer_buffer_take(conn, turn).await? else {
        return Ok(());
    };
    for m in &msgs {
        let text = m.get("text").and_then(Value::as_str).unwrap_or("");
        if text.trim().is_empty() {
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
        conv::append_event(conn, conversation, Some(turn), "text", &p).await?;
    }
    Ok(())
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
    crate::domains::schedules::fire::on_turn_finished(conn, &row).await?;
    crate::domains::webhooks::on_turn_finished(conn, &row).await?;
    crate::domains::peer_tasks::service::on_turn_finished(conn, &row).await?;
    crate::domains::channels::mirror::on_turn_finished(conn, &row).await?;
    Ok(Some(row))
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
    // (FIX-115) The answer text the agent had already written is kept, before the error.
    drain_answer(conn, conversation, turn).await?;
    conv::append_event(
        conn,
        conversation,
        Some(turn),
        "error",
        &json!({"turn_id": turn, "code": code, "message": message}),
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
