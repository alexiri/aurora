use super::dto::{Conversation, EventsPage, MessageAccepted, TranscriptEvent, Turn};
use crate::app::AppState;
use crate::audit::{AuditActor, Outcome};
use crate::http::auth::Caller;
use aurora_core::limits::{ATTACH_IN_COUNT, TURN_TEXT_MAX};
use aurora_core::{ApiError, FieldError};
use aurora_db::DbError;
use aurora_db::authz::{self, WorkspaceAuth};
use aurora_db::bus::{self, Channel};
use aurora_db::repo::agents::{self, AgentRow};
use aurora_db::repo::audit::{self as audit_repo, NewAudit};
use aurora_db::repo::conversations::{self as convs, ConversationRow, NewConversation};
use aurora_db::repo::lifecycle;
use aurora_db::repo::transcript;
use aurora_db::repo::turns::{self, Created, NewTurn, TurnRow};
use aurora_db::repo::users;
use aurora_db::repo::workspaces;
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

/// Largest slice of turn text copied into the `turn.queued` event (payload bound is 64 KiB).
const EVENT_TEXT_MAX_BYTES: usize = 48 * 1024;
const EXTERNAL_REF_MAX: usize = 128;
const TITLE_MAX: usize = 200;

fn bad(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_request("Invalid request.", vec![FieldError::new(path, msg)])
}

fn conv_404() -> ApiError {
    ApiError::not_found("Conversation not found.")
}

// ---------------------------------------------------------------- access

/// Agent visible to the caller (`can_use` on its workspace; key confinement applies).
pub async fn load_agent(state: &AppState, c: &Caller, id: Uuid) -> Result<AgentRow, ApiError> {
    let a = agents::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    c.workspace(state, a.workspace_id).await?;
    Ok(a)
}

/// Conversation visible to the caller (`can_see_conversation`, docs/04; RFE-009). A direct
/// conversation is visible to its user and to workspace owners; anyone else gets 404. This is
/// the predicate for reading, abort and send-now; writing needs [`load_conversation_for_write`].
pub async fn load_conversation(
    state: &AppState,
    c: &Caller,
    id: Uuid,
) -> Result<ConversationRow, ApiError> {
    Ok(load_conversation_auth(state, c, id).await?.0)
}

pub async fn load_conversation_auth(
    state: &AppState,
    c: &Caller,
    id: Uuid,
) -> Result<(ConversationRow, WorkspaceAuth), ApiError> {
    let row = convs::get(&state.pool, id).await?.ok_or_else(conv_404)?;
    let auth = c.workspace(state, row.workspace_id).await?;
    if !authz::can_see_conversation(&auth, c.user_id, &row.kind, row.direct_user_id) {
        return Err(conv_404());
    }
    Ok((row, auth))
}

/// Like [`load_conversation`] for a *read* (transcript, events, stream, attachment bytes): an
/// owner who is not the direct user also writes the best-effort `conversation.owner_view` audit
/// row (once per actor, conversation and hour).
pub async fn read_conversation(
    state: &AppState,
    c: &Caller,
    id: Uuid,
) -> Result<ConversationRow, ApiError> {
    let (row, _) = load_conversation_auth(state, c, id).await?;
    if row.kind == "direct" && row.direct_user_id != Some(c.user_id) {
        note_owner_view(state, c, &row).await;
    }
    Ok(row)
}

async fn note_owner_view(state: &AppState, c: &Caller, row: &ConversationRow) {
    if let Err(e) = try_note_owner_view(state, c, row).await {
        // Best-effort (docs/04 §Audit): never fails the read.
        tracing::warn!(error = %e, "conversation.owner_view audit failed");
    }
}

/// Once per actor, conversation and hour: the check and the insert run under an advisory lock so
/// concurrent reads (transcript + events + stream) write a single row.
async fn try_note_owner_view(
    state: &AppState,
    c: &Caller,
    row: &ConversationRow,
) -> Result<(), DbError> {
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("owner_view:{}:{}", c.user_id, row.id))
        .execute(&mut *tx)
        .await?;
    let seen: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM audit_log WHERE workspace_id = $1 AND actor_id = $2
           AND action = 'conversation.owner_view' AND target_id = $3
           AND created_at > now() - interval '1 hour')",
    )
    .bind(row.workspace_id)
    .bind(c.user_id)
    .bind(row.id.to_string())
    .fetch_one(&mut *tx)
    .await?;
    if seen {
        return Ok(());
    }
    let org = workspaces::get(&mut *tx, row.workspace_id)
        .await?
        .map(|w| w.org_id);
    let actor = AuditActor::from_caller(c);
    audit_repo::insert(
        &mut *tx,
        &NewAudit {
            org_id: org,
            workspace_id: Some(row.workspace_id),
            actor_kind: actor.kind,
            actor_id: actor.id,
            actor_label: &actor.label,
            action: "conversation.owner_view",
            target_kind: Some("conversation"),
            target_id: Some(&row.id.to_string()),
            outcome: Outcome::Success.as_str(),
            source: actor.source,
            ip: actor.ip,
            metadata: &json!({"agent_id": row.agent_id, "direct_user_id": row.direct_user_id}),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// A conversation the caller may *write* into (post a message, upload an attachment): visible,
/// and for `direct` conversations the direct user alone. An owner who can see it gets 403
/// `direct_conversation_forbidden`.
pub async fn load_conversation_for_write(
    state: &AppState,
    c: &Caller,
    id: Uuid,
) -> Result<ConversationRow, ApiError> {
    let row = load_conversation(state, c, id).await?;
    if !authz::can_write_conversation(c.user_id, &row.kind, row.direct_user_id) {
        return Err(ApiError::new(
            axum::http::StatusCode::FORBIDDEN,
            "direct_conversation_forbidden",
            "Only the person in a direct conversation can write to it.",
        ));
    }
    Ok(row)
}

// ---------------------------------------------------------------- conversations

pub async fn direct(
    state: &AppState,
    c: &Caller,
    agent_id: Uuid,
) -> Result<Conversation, ApiError> {
    let agent = load_agent(state, c, agent_id).await?;
    let ext = format!("user:{}", c.user_id);
    let mut conn = state.pool.acquire().await.map_err(DbError::from)?;
    let (row, _) = convs::get_or_create(
        &mut conn,
        &NewConversation {
            agent_id,
            workspace_id: agent.workspace_id,
            kind: "direct",
            external_ref: &ext,
            direct_user_id: Some(c.user_id),
            title: None,
            origin_conversation_id: None,
        },
    )
    .await?;
    let active = turns::active_turn(&mut *conn, row.id).await?;
    Ok(Conversation::from_row(row, active))
}

pub async fn api_conversation(
    state: &AppState,
    c: &Caller,
    agent_id: Uuid,
    external_ref: &str,
    title: Option<&str>,
) -> Result<(Conversation, bool), ApiError> {
    if c.is_session() {
        return Err(ApiError::forbidden(
            "API conversations are created with an API key.",
        ));
    }
    let ext = external_ref.trim();
    if ext.is_empty() || ext.chars().count() > EXTERNAL_REF_MAX || ext.chars().any(char::is_control)
    {
        return Err(bad("/external_ref", "must be 1-128 characters"));
    }
    let title = title.map(str::trim).filter(|t| !t.is_empty());
    if title.is_some_and(|t| t.chars().count() > TITLE_MAX) {
        return Err(bad("/title", "must be at most 200 characters"));
    }
    let agent = load_agent(state, c, agent_id).await?;
    let mut conn = state.pool.acquire().await.map_err(DbError::from)?;
    let (row, created) = convs::get_or_create(
        &mut conn,
        &NewConversation {
            agent_id,
            workspace_id: agent.workspace_id,
            kind: "api",
            external_ref: ext,
            direct_user_id: None,
            title,
            origin_conversation_id: None,
        },
    )
    .await?;
    let active = turns::active_turn(&mut *conn, row.id).await?;
    Ok((Conversation::from_row(row, active), created))
}

pub async fn get(state: &AppState, c: &Caller, id: Uuid) -> Result<Conversation, ApiError> {
    let row = read_conversation(state, c, id).await?;
    let active = turns::active_turn(&state.pool, row.id).await?;
    Ok(Conversation::from_row(row, active))
}

pub async fn events(
    state: &AppState,
    c: &Caller,
    id: Uuid,
    after_seq: Option<i64>,
    limit: Option<u32>,
) -> Result<EventsPage, ApiError> {
    let after = after_seq.unwrap_or(0);
    if after < 0 {
        return Err(bad("/after_seq", "must be >= 0"));
    }
    let limit = limit.unwrap_or(200);
    if !(1..=500).contains(&limit) {
        return Err(bad("/limit", "must be between 1 and 500"));
    }
    let conv = read_conversation(state, c, id).await?;
    let rows = transcript::list_after(&state.pool, id, after, i64::from(limit)).await?;
    // Page cursor: the last returned seq, or `after` when nothing is newer.
    let last_seq = rows
        .last()
        .map_or(after.min(conv.last_seq).max(0), |r| r.seq);
    Ok(EventsPage {
        items: rows.into_iter().map(TranscriptEvent::from).collect(),
        last_seq,
    })
}

// ---------------------------------------------------------------- turns

/// Input for [`create_turn`], shared by the dashboard/API door and the other doors (BE-052+).
pub struct NewMessage<'a> {
    /// Stored door object, e.g. `{"kind":"web"}` (docs/03).
    pub door: Value,
    /// `person` | `bot` | `agent` | `system`
    pub speaker_kind: &'a str,
    pub speaker_user_id: Option<Uuid>,
    pub speaker_label: &'a str,
    pub text: &'a str,
    pub source_event_id: Option<&'a str>,
    pub attachment_ids: &'a [Uuid],
}

fn clip(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

pub fn check_text(text: &str) -> Result<(), ApiError> {
    if text.trim().is_empty() {
        return Err(bad("/text", "must not be empty"));
    }
    if text.chars().count() > TURN_TEXT_MAX {
        return Err(ApiError::too_large("Message text is too large."));
    }
    Ok(())
}

/// Create a turn (or a follow-up when one is active) inside the caller's transaction: lock the
/// conversation, insert (the partial unique index arbitrates races), bind attachments, append the
/// durable `turn.queued` event and `NOTIFY aurora_work` (both delivered on commit).
///
/// Bot/agent speakers are subject to the app-turn cap (07): use [`create_turn_capped`] for them
/// so a refusal commits its durable notice. Here a refusal is a 409 `app_turn_cap_reached`
/// (which rolls the transaction back), so only person/system speakers belong here.
pub async fn create_turn_tx(
    conn: &mut PgConnection,
    conv: &ConversationRow,
    m: &NewMessage<'_>,
) -> Result<Created, ApiError> {
    create_turn_capped(conn, conv, m).await?.ok_or_else(|| {
        ApiError::conflict(
            "app_turn_cap_reached",
            "Too many consecutive automated turns; a person must continue the conversation.",
        )
    })
}

/// As [`create_turn_tx`], applying the app-turn cap (docs/07): a bot/agent turn increments
/// `conversations.app_turn_streak`; at `APP_TURN_CAP` it is refused with a notice; a
/// person-authored turn resets the streak. System turns neither count nor reset. `None`: the
/// cap refused the turn (a durable `app_turn_cap_reached` notice was appended; no turn exists).
pub async fn create_turn_capped(
    conn: &mut PgConnection,
    conv: &ConversationRow,
    m: &NewMessage<'_>,
) -> Result<Option<Created>, ApiError> {
    if !convs::lock(&mut *conn, conv.id).await? {
        return Err(conv_404());
    }
    match m.speaker_kind {
        "person" => {
            convs::reset_app_turn_streak(&mut *conn, conv.id).await?;
            // A person-authored message unarchives the conversation (RFE-010).
            lifecycle::set_archived(&mut *conn, conv.id, false, None).await?;
        }
        "bot" | "agent" => {
            let streak = convs::get(&mut *conn, conv.id)
                .await?
                .map_or(0, |c| c.app_turn_streak);
            if u32::try_from(streak).unwrap_or(0) >= aurora_core::limits::APP_TURN_CAP {
                transcript::append_durable_event(
                    &mut *conn,
                    conv.id,
                    None,
                    "notice",
                    &json!({"code": "app_turn_cap_reached",
                        "text": "Automated messages were paused: too many consecutive bot or agent turns. A person must write or continue the conversation."}),
                )
                .await?;
                return Ok(None);
            }
            convs::bump_app_turn_streak(&mut *conn, conv.id).await?;
        }
        _ => {}
    }
    let n = NewTurn {
        conversation_id: conv.id,
        agent_id: conv.agent_id,
        door: &m.door,
        speaker_kind: m.speaker_kind,
        speaker_user_id: m.speaker_user_id,
        speaker_label: m.speaker_label,
        text: m.text,
        source_event_id: m.source_event_id,
    };
    let created = turns::create(&mut *conn, &n).await?;
    let row = match &created {
        Created::Turn(r) | Created::Followup(r) => r,
    };
    if !m.attachment_ids.is_empty() {
        let bound = turns::bind_attachments(&mut *conn, conv.id, row.id, m.attachment_ids).await?;
        if bound != m.attachment_ids.len() as u64 {
            return Err(bad(
                "/attachment_ids",
                "unknown, already used, or not uploaded to this conversation",
            ));
        }
    }
    let mut payload = json!({
        "turn_id": row.id,
        "door": super::dto::door_name(&row.door),
        "speaker_label": row.speaker_label,
        "text": clip(&row.text, EVENT_TEXT_MAX_BYTES),
        "attachments": m.attachment_ids,
    });
    if let Some(parent) = row.parent_turn_id.filter(|_| row.role == "followup") {
        payload["followup_of"] = json!(parent);
    }
    transcript::append_durable_event(&mut *conn, conv.id, Some(row.id), "turn.queued", &payload)
        .await?;
    notify_work(&mut *conn).await?;
    Ok(Some(created))
}

/// `NOTIFY aurora_work` (delivered on commit when inside a transaction).
pub async fn notify_work(conn: &mut PgConnection) -> Result<(), ApiError> {
    bus::notify(&mut *conn, Channel::Work, &json!({}))
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "work notify failed");
            ApiError::internal("Notification failed.")
        })
}

/// Append one durable event (own helper for BE-052): returns its seq.
pub async fn append_event(
    conn: &mut PgConnection,
    conversation_id: Uuid,
    turn_id: Option<Uuid>,
    ty: &str,
    payload: &Value,
) -> Result<i64, ApiError> {
    Ok(transcript::append_durable_event(conn, conversation_id, turn_id, ty, payload).await?)
}

/// `dispatched` -> `running` plus the durable `turn.started` event; fenced (`None` on replay).
pub async fn start_turn(
    conn: &mut PgConnection,
    turn_id: Uuid,
    boot_id: Option<Uuid>,
) -> Result<Option<TurnRow>, ApiError> {
    let Some(t) = turns::get(&mut *conn, turn_id).await? else {
        return Ok(None);
    };
    convs::lock(&mut *conn, t.conversation_id).await?;
    let Some(row) = turns::set_running(&mut *conn, turn_id, boot_id).await? else {
        return Ok(None);
    };
    transcript::append_durable_event(
        &mut *conn,
        row.conversation_id,
        Some(turn_id),
        "turn.started",
        &json!({"turn_id": turn_id}),
    )
    .await?;
    Ok(Some(row))
}

/// End a turn (`done` | `failed` | `aborted`), append `turn.finished`, and promote the turn's
/// still-`joining` follow-ups (the live turn ended first). Fenced: `None` when the turn was
/// already terminal, so replays are no-ops (I-25).
pub async fn finish_turn(
    conn: &mut PgConnection,
    turn_id: Uuid,
    status: &str,
    error_code: Option<&str>,
    error_message: Option<&str>,
    usage: Option<&Value>,
) -> Result<Option<TurnRow>, ApiError> {
    let Some(t) = turns::get(&mut *conn, turn_id).await? else {
        return Ok(None);
    };
    convs::lock(&mut *conn, t.conversation_id).await?;
    let Some(row) = turns::finish(
        &mut *conn,
        turn_id,
        status,
        error_code,
        error_message,
        usage,
    )
    .await?
    else {
        return Ok(None);
    };
    let mut payload = json!({"turn_id": turn_id, "status": status});
    if let Some(code) = &row.error_code {
        payload["error_code"] = json!(code);
    }
    if let Some(u) = &row.usage {
        payload["usage"] = u.clone();
    }
    transcript::append_durable_event(
        &mut *conn,
        row.conversation_id,
        Some(turn_id),
        "turn.finished",
        &payload,
    )
    .await?;
    promote_followups(&mut *conn, turn_id).await?;
    Ok(Some(row))
}

/// Promote every `joining` follow-up of `parent` into queued turns (the first becomes the active
/// turn; later ones are re-parented under it). Returns the newly queued turns.
pub async fn promote_followups(
    conn: &mut PgConnection,
    parent: Uuid,
) -> Result<Vec<TurnRow>, ApiError> {
    let mut out = Vec::new();
    for f in turns::joining_of(&mut *conn, parent).await? {
        if let Some(t) = promote_one(&mut *conn, &f).await? {
            out.push(t);
        }
    }
    if !out.is_empty() {
        notify_work(&mut *conn).await?;
    }
    Ok(out)
}

/// Promote one `joining` follow-up (fenced by `promoted_at`) and append the durable
/// `turn.queued` of the new turn with `promoted_from` (C-26).
pub async fn promote_one(
    conn: &mut PgConnection,
    f: &TurnRow,
) -> Result<Option<TurnRow>, ApiError> {
    let Some(t) = turns::promote_followup(&mut *conn, f.id).await? else {
        return Ok(None);
    };
    let attachments = turns::get(&mut *conn, t.id)
        .await?
        .map(|r| r.attachment_ids)
        .unwrap_or_default();
    transcript::append_durable_event(
        &mut *conn,
        t.conversation_id,
        Some(t.id),
        "turn.queued",
        &json!({
            "turn_id": t.id,
            "door": super::dto::door_name(&t.door),
            "speaker_label": t.speaker_label,
            "text": clip(&t.text, EVENT_TEXT_MAX_BYTES),
            "attachments": attachments,
            "promoted_from": f.id,
        }),
    )
    .await?;
    Ok(Some(t))
}

pub(super) async fn speaker_label(state: &AppState, c: &Caller) -> Result<String, ApiError> {
    let name = users::find_by_id(&state.pool, c.user_id)
        .await?
        .map(|u| u.display_name)
        .unwrap_or_default();
    let label = aurora_core::sanitize::label(&name);
    Ok(if label.is_empty() {
        "User".to_owned()
    } else {
        label
    })
}

pub async fn post_message(
    state: &AppState,
    c: &Caller,
    conversation_id: Uuid,
    text: &str,
    attachment_ids: &[Uuid],
) -> Result<MessageAccepted, ApiError> {
    check_text(text)?;
    let conv = load_conversation_for_write(state, c, conversation_id).await?;
    let agent = agents::get(&state.pool, conv.agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    if agent.kind == "hosted" && !state.config.hosted.enabled {
        return Err(ApiError::unprocessable(
            "hosted_disabled",
            "Hosted agents are disabled on this instance.",
        ));
    }
    let mut ids = attachment_ids.to_vec();
    ids.sort();
    ids.dedup();
    if ids.len() > ATTACH_IN_COUNT as usize {
        return Err(ApiError::unprocessable(
            "too_many_attachments",
            "Too many attachments on one message.",
        ));
    }
    let label = speaker_label(state, c).await?;
    let door = json!({"kind": if c.is_session() { "web" } else { "api" }});
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let created = create_turn_tx(
        &mut tx,
        &conv,
        &NewMessage {
            door,
            speaker_kind: "person",
            speaker_user_id: Some(c.user_id),
            speaker_label: &label,
            text,
            source_event_id: None,
            attachment_ids: &ids,
        },
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(match created {
        Created::Turn(t) => MessageAccepted {
            turn: Some(t.into()),
            followup: None,
        },
        Created::Followup(f) => MessageAccepted {
            turn: None,
            followup: Some(f.into()),
        },
    })
}

pub(super) async fn load_turn(state: &AppState, c: &Caller, id: Uuid) -> Result<TurnRow, ApiError> {
    let t = turns::get(&state.pool, id)
        .await?
        .filter(|t| t.role == "turn")
        .ok_or_else(|| ApiError::not_found("Turn not found."))?;
    load_conversation(state, c, t.conversation_id)
        .await
        .map_err(|_| ApiError::not_found("Turn not found."))?;
    Ok(t)
}

fn finished() -> ApiError {
    ApiError::conflict("turn_finished", "The turn has already finished.")
}

/// Abort: a queued turn becomes `aborted` immediately (with `turn.finished`); a dispatched/running
/// turn gets `abort_requested_at` and ends on the supervisor's terminal event or stall recovery.
pub async fn abort(state: &AppState, c: &Caller, id: Uuid) -> Result<Turn, ApiError> {
    let t = load_turn(state, c, id).await?;
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    convs::lock(&mut tx, t.conversation_id).await?;
    let Some(row) = turns::request_abort(&mut tx, id).await? else {
        return Err(finished());
    };
    let row = if row.status == "queued" {
        finish_turn(&mut tx, id, "aborted", None, None, None)
            .await?
            .unwrap_or(row)
    } else {
        notify_work(&mut tx).await?;
        row
    };
    let out = turns::get(&mut *tx, row.id).await?.unwrap_or(row);
    tx.commit().await.map_err(DbError::from)?;
    Ok(out.into())
}

/// Send now: sets `send_now_requested_at` (delivered as `turn.send_now` by the engine).
pub async fn send_now(state: &AppState, c: &Caller, id: Uuid) -> Result<Turn, ApiError> {
    let t = load_turn(state, c, id).await?;
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    convs::lock(&mut tx, t.conversation_id).await?;
    let Some(row) = turns::request_send_now(&mut tx, id).await? else {
        return Err(finished());
    };
    notify_work(&mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(row.into())
}

/// `POST /conversations/{id}/continue` (C-27): reset the app-turn streak and append the durable
/// `app_turn_cap_reset` notice.
pub async fn continue_conversation(
    state: &AppState,
    c: &Caller,
    id: Uuid,
) -> Result<Conversation, ApiError> {
    load_conversation_for_write(state, c, id).await?;
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    if !convs::lock(&mut tx, id).await? {
        return Err(conv_404());
    }
    convs::reset_app_turn_streak(&mut tx, id).await?;
    transcript::append_durable_event(
        &mut tx,
        id,
        None,
        "notice",
        &json!({"code": "app_turn_cap_reset",
            "text": "Automated messages were resumed by a person."}),
    )
    .await?;
    let row = convs::get(&mut *tx, id).await?.ok_or_else(conv_404)?;
    let active = turns::active_turn(&mut *tx, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(Conversation::from_row(row, active))
}
