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
    let Some(t) = aurora_db::repo::turns::get(&mut *conn, turn).await? else {
        return Ok(None);
    };
    if t.role != "turn" || !matches!(t.status.as_str(), "queued" | "dispatched" | "running") {
        return Ok(None);
    }
    aurora_db::repo::conversations::lock(&mut *conn, conversation).await?;
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
