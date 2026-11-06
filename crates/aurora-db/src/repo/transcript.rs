//! Transcript events: durable append (seq bump + insert in one transaction) and reads.
//! Ephemeral types (`text.delta`, `thinking.delta`, `activity`) are never stored.

use crate::DbError;
use crate::bus::{self, BusError, Channel};
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

/// Serialized payload bound (docs/03 `transcript_events.payload`).
pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024;

/// Event types that are never persisted.
pub const EPHEMERAL_TYPES: [&str; 3] = ["text.delta", "thinking.delta", "activity"];

pub fn is_ephemeral(ty: &str) -> bool {
    EPHEMERAL_TYPES.contains(&ty)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EventRow {
    pub seq: i64,
    pub turn_id: Option<Uuid>,
    #[sqlx(rename = "type")]
    pub ty: String,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

fn check(constraint: &str) -> DbError {
    DbError::CheckViolation {
        constraint: Some(constraint.to_owned()),
    }
}

/// Append one durable event: `last_seq = last_seq + 1` (row lock, so commit order equals seq
/// order) and the insert happen on `conn`, which MUST be inside the caller's transaction.
/// `NOTIFY aurora_conversation {c, s}` is queued and delivered on commit. Returns the seq.
pub async fn append_durable_event(
    conn: &mut PgConnection,
    conversation_id: Uuid,
    turn_id: Option<Uuid>,
    ty: &str,
    payload: &serde_json::Value,
) -> Result<i64, DbError> {
    if is_ephemeral(ty) {
        return Err(check("transcript_event_ephemeral"));
    }
    if ty.is_empty() || ty.len() > 64 || payload.to_string().len() > MAX_PAYLOAD_BYTES {
        return Err(check("transcript_event_bounds"));
    }
    let seq: i64 = sqlx::query_scalar(
        "UPDATE conversations SET last_seq = last_seq + 1, last_activity_at = now()
         WHERE id = $1 RETURNING last_seq",
    )
    .bind(conversation_id)
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO transcript_events (conversation_id, seq, turn_id, type, payload)
         VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(conversation_id)
    .bind(seq)
    .bind(turn_id)
    .bind(ty)
    .bind(payload)
    .execute(&mut *conn)
    .await?;
    bus::notify(
        &mut *conn,
        Channel::Conversation,
        &serde_json::json!({"c": conversation_id, "s": seq}),
    )
    .await
    .map_err(|e| match e {
        BusError::Db(e) => e.into(),
        other => DbError::Other(sqlx::Error::Protocol(other.to_string())),
    })?;
    Ok(seq)
}

/// Durable events with `seq > after_seq`, ascending.
pub async fn list_after<'e, E: PgExecutor<'e>>(
    exec: E,
    conversation_id: Uuid,
    after_seq: i64,
    limit: i64,
) -> Result<Vec<EventRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT seq, turn_id, type, payload, created_at FROM transcript_events
         WHERE conversation_id = $1 AND seq > $2 ORDER BY seq LIMIT $3",
    )
    .bind(conversation_id)
    .bind(after_seq)
    .bind(limit)
    .fetch_all(exec)
    .await?)
}

/// Emit an ephemeral delta on `aurora_delta` (`t` is the turn id; `m` the optional message id).
pub async fn notify_delta<'e, E: PgExecutor<'e>>(
    exec: E,
    payload: &serde_json::Value,
) -> Result<(), BusError> {
    bus::notify(exec, Channel::Delta, payload).await
}
