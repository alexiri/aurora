//! Turns and follow-ups (docs/03 §6, docs/10 §Turns). Both live in `turns` (`role`).
//!
//! Lock order for writers that touch several rows: conversation row, then turn rows.
//! Every state change is **fenced** on the expected current status and returns `None` when the
//! fence no longer holds (idempotent replays, I-25).

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::{Connection, PgConnection, PgExecutor};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TurnRow {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub agent_id: Uuid,
    pub role: String,
    pub status: String,
    pub parent_turn_id: Option<Uuid>,
    pub door: serde_json::Value,
    pub speaker_kind: String,
    pub speaker_user_id: Option<Uuid>,
    pub speaker_label: String,
    pub text: String,
    pub source_event_id: Option<String>,
    pub abort_requested_at: Option<DateTime<Utc>>,
    pub send_now_requested_at: Option<DateTime<Utc>>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub usage: Option<serde_json::Value>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub attachment_ids: Vec<Uuid>,
}

const COLS: &str = "turns.id, turns.conversation_id, turns.agent_id, turns.role, turns.status, \
    turns.parent_turn_id, turns.door, turns.speaker_kind, turns.speaker_user_id, \
    turns.speaker_label, turns.text, turns.source_event_id, turns.abort_requested_at, \
    turns.send_now_requested_at, turns.error_code, turns.error_message, turns.usage, \
    turns.started_at, turns.finished_at, turns.created_at, \
    COALESCE((SELECT array_agg(a.id ORDER BY a.created_at, a.id) FROM attachments a \
        WHERE a.turn_id = turns.id), '{}'::uuid[]) AS attachment_ids";

const ACTIVE: &str = "('queued','dispatched','running')";

pub struct NewTurn<'a> {
    pub conversation_id: Uuid,
    pub agent_id: Uuid,
    pub door: &'a serde_json::Value,
    /// `person` | `bot` | `agent` | `system`
    pub speaker_kind: &'a str,
    pub speaker_user_id: Option<Uuid>,
    pub speaker_label: &'a str,
    pub text: &'a str,
    pub source_event_id: Option<&'a str>,
}

pub enum Created {
    Turn(TurnRow),
    Followup(TurnRow),
}

pub async fn get<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<Option<TurnRow>, DbError> {
    Ok(
        sqlx::query_as(&format!("SELECT {COLS} FROM turns WHERE id = $1"))
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}

/// The active `turn` (queued/dispatched/running) of a conversation.
pub async fn active_turn<'e, E: PgExecutor<'e>>(
    exec: E,
    conversation_id: Uuid,
) -> Result<Option<TurnRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM turns WHERE conversation_id = $1 AND role = 'turn' AND status IN {ACTIVE}"
    ))
    .bind(conversation_id)
    .fetch_optional(exec)
    .await?)
}

pub async fn active_turns<'e, E: PgExecutor<'e>>(
    exec: E,
    conversation_ids: &[Uuid],
) -> Result<Vec<TurnRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM turns WHERE conversation_id = ANY($1) AND role = 'turn' AND status IN {ACTIVE}"
    ))
    .bind(conversation_ids)
    .fetch_all(exec)
    .await?)
}

async fn insert(
    conn: &mut PgConnection,
    n: &NewTurn<'_>,
    role: &str,
    status: &str,
    parent: Option<Uuid>,
) -> Result<TurnRow, DbError> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO turns (id, conversation_id, agent_id, role, status, parent_turn_id, door,
            speaker_kind, speaker_user_id, speaker_label, text, source_event_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
    )
    .bind(id)
    .bind(n.conversation_id)
    .bind(n.agent_id)
    .bind(role)
    .bind(status)
    .bind(parent)
    .bind(n.door)
    .bind(n.speaker_kind)
    .bind(n.speaker_user_id)
    .bind(n.speaker_label)
    .bind(n.text)
    .bind(n.source_event_id)
    .execute(&mut *conn)
    .await?;
    get(&mut *conn, id).await?.ok_or(DbError::NotFound)
}

fn is_active_conflict(e: &DbError) -> bool {
    matches!(e, DbError::UniqueViolation { constraint: Some(c) } if c == "turns_one_active_per_conversation_uq")
}

/// Insert a `queued` turn; if the partial unique index rejects it (an active turn exists) insert a
/// `joining` follow-up under that turn instead (the index is the race arbiter). Emits
/// `NOTIFY aurora_work` on commit. A `source_event_id` duplicate surfaces as `UniqueViolation`.
pub async fn create(conn: &mut PgConnection, n: &NewTurn<'_>) -> Result<Created, DbError> {
    for _ in 0..4 {
        let mut sp = conn.begin().await?;
        match insert(&mut sp, n, "turn", "queued", None).await {
            Ok(row) => {
                sp.commit().await?;
                return Ok(Created::Turn(row));
            }
            Err(e) if is_active_conflict(&e) => sp.rollback().await?,
            Err(e) => return Err(e),
        }
        // Share-lock the live turn so a concurrent finish waits for (and then sees) the follow-up.
        let parent: Option<Uuid> = sqlx::query_scalar(&format!(
            "SELECT id FROM turns WHERE conversation_id = $1 AND role = 'turn' AND status IN {ACTIVE} FOR SHARE"
        ))
        .bind(n.conversation_id)
        .fetch_optional(&mut *conn)
        .await?;
        if let Some(p) = parent {
            let row = insert(conn, n, "followup", "joining", Some(p)).await?;
            return Ok(Created::Followup(row));
        }
        // The live turn finished in between: retry as a fresh turn.
    }
    Err(DbError::Other(sqlx::Error::Protocol(
        "turn creation did not settle".into(),
    )))
}

/// `dispatched` -> `running` (supervisor `turn.started`); fenced.
pub async fn set_running(
    conn: &mut PgConnection,
    id: Uuid,
    boot_id: Option<Uuid>,
) -> Result<Option<TurnRow>, DbError> {
    let done = sqlx::query(
        "UPDATE turns SET status = 'running', started_at = COALESCE(started_at, now()),
            last_progress_at = now(), boot_id = COALESCE($2, boot_id), updated_at = now()
         WHERE id = $1 AND role = 'turn' AND status = 'dispatched'",
    )
    .bind(id)
    .bind(boot_id)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(None);
    }
    get(conn, id).await
}

/// End an active turn (`done` | `failed` | `aborted`); fenced on the turn still being active, so a
/// replayed terminal report returns `None`.
pub async fn finish(
    conn: &mut PgConnection,
    id: Uuid,
    status: &str,
    error_code: Option<&str>,
    error_message: Option<&str>,
    usage: Option<&serde_json::Value>,
) -> Result<Option<TurnRow>, DbError> {
    if !matches!(status, "done" | "failed" | "aborted") {
        return Err(DbError::CheckViolation {
            constraint: Some("turns_finish_status".into()),
        });
    }
    let done = sqlx::query(&format!(
        "UPDATE turns SET status = $2, error_code = $3, error_message = $4, usage = COALESCE($5, usage),
            finished_at = now(), updated_at = now()
         WHERE id = $1 AND role = 'turn' AND status IN {ACTIVE}"
    ))
    .bind(id)
    .bind(status)
    .bind(error_code)
    .bind(error_message)
    .bind(usage)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(None);
    }
    get(conn, id).await
}

/// Mark an active turn abort-requested (idempotent); returns the row or `None` when finished.
pub async fn request_abort(conn: &mut PgConnection, id: Uuid) -> Result<Option<TurnRow>, DbError> {
    let done = sqlx::query(&format!(
        "UPDATE turns SET abort_requested_at = COALESCE(abort_requested_at, now()), updated_at = now()
         WHERE id = $1 AND role = 'turn' AND status IN {ACTIVE}"
    ))
    .bind(id)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(None);
    }
    get(conn, id).await
}

/// Set the send-now flag on an active turn (idempotent).
pub async fn request_send_now(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<TurnRow>, DbError> {
    let done = sqlx::query(&format!(
        "UPDATE turns SET send_now_requested_at = COALESCE(send_now_requested_at, now()), updated_at = now()
         WHERE id = $1 AND role = 'turn' AND status IN {ACTIVE}"
    ))
    .bind(id)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(None);
    }
    get(conn, id).await
}

/// `joining` follow-ups of a turn, oldest first, row-locked.
pub async fn joining_of(conn: &mut PgConnection, parent: Uuid) -> Result<Vec<TurnRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM turns WHERE parent_turn_id = $1 AND role = 'followup' AND status = 'joining'
         ORDER BY created_at, id FOR UPDATE"
    ))
    .bind(parent)
    .fetch_all(&mut *conn)
    .await?)
}

/// `joining` -> `joined` (supervisor `message.joined`); fenced.
pub async fn mark_joined(conn: &mut PgConnection, id: Uuid) -> Result<Option<TurnRow>, DbError> {
    let done = sqlx::query(
        "UPDATE turns SET status = 'joined', updated_at = now()
         WHERE id = $1 AND role = 'followup' AND status = 'joining'",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(None);
    }
    get(conn, id).await
}

/// Stamp `steer_delivered_at` once (engine `turn.message` delivery); true when newly set.
pub async fn mark_steer_delivered(conn: &mut PgConnection, id: Uuid) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE turns SET steer_delivered_at = now(), updated_at = now()
         WHERE id = $1 AND role = 'followup' AND status = 'joining' AND steer_delivered_at IS NULL",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Promote a `joining` follow-up into a new `queued` turn (one promotion per follow-up, fenced by
/// `promoted_at`). Returns the new turn, or `None` when already promoted/joined or when another
/// turn became active meanwhile (then the follow-up is re-parented under that turn and stays
/// `joining`).
pub async fn promote_followup(
    conn: &mut PgConnection,
    followup_id: Uuid,
) -> Result<Option<TurnRow>, DbError> {
    let mut sp = conn.begin().await?;
    let done = sqlx::query(
        "UPDATE turns SET status = 'promoted', promoted_at = now(), updated_at = now()
         WHERE id = $1 AND role = 'followup' AND status = 'joining' AND promoted_at IS NULL",
    )
    .bind(followup_id)
    .execute(&mut *sp)
    .await?;
    if done.rows_affected() == 0 {
        sp.rollback().await?;
        return Ok(None);
    }
    let f = get(&mut *sp, followup_id).await?.ok_or(DbError::NotFound)?;
    let n = NewTurn {
        conversation_id: f.conversation_id,
        agent_id: f.agent_id,
        door: &f.door,
        speaker_kind: &f.speaker_kind,
        speaker_user_id: f.speaker_user_id,
        speaker_label: &f.speaker_label,
        text: &f.text,
        source_event_id: None,
    };
    match insert(&mut sp, &n, "turn", "queued", None).await {
        Ok(row) => {
            sp.commit().await?;
            Ok(Some(row))
        }
        Err(e) if is_active_conflict(&e) => {
            sp.rollback().await?;
            sqlx::query(&format!(
                "UPDATE turns SET parent_turn_id = (SELECT id FROM turns WHERE conversation_id = $2
                    AND role = 'turn' AND status IN {ACTIVE}), updated_at = now() WHERE id = $1"
            ))
            .bind(followup_id)
            .bind(f.conversation_id)
            .execute(&mut *conn)
            .await?;
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// Bind pending inbound attachments of a conversation to a turn; returns how many were bound.
pub async fn bind_attachments(
    conn: &mut PgConnection,
    conversation_id: Uuid,
    turn_id: Uuid,
    ids: &[Uuid],
) -> Result<u64, DbError> {
    let done = sqlx::query(
        "UPDATE attachments SET turn_id = $3, status = 'bound', updated_at = now()
         WHERE id = ANY($1) AND conversation_id = $2 AND direction = 'in' AND status = 'pending'
           AND turn_id IS NULL AND purged_at IS NULL",
    )
    .bind(ids)
    .bind(conversation_id)
    .bind(turn_id)
    .execute(&mut *conn)
    .await?;
    Ok(done.rows_affected())
}

/// The turn (or follow-up) a connector event already created in this conversation.
pub async fn by_source_event(
    conn: &mut PgConnection,
    conversation_id: Uuid,
    source_event_id: &str,
) -> Result<Option<TurnRow>, DbError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM turns WHERE conversation_id = $1 AND source_event_id = $2"
    ))
    .bind(conversation_id)
    .bind(source_event_id)
    .fetch_optional(conn)
    .await?)
}
