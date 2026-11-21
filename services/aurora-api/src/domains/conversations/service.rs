use super::dto::{
    Conversation, EventsPage, MessageAccepted, PurgedAttachment, TranscriptEvent, Turn,
};
use crate::app::AppState;
use crate::audit::{AuditActor, Outcome};
use crate::http::auth::Caller;
use aurora_core::ApiError;
use aurora_core::limits::{ATTACH_IN_COUNT, TURN_TEXT_MAX};
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
    ApiError::invalid_field("Invalid request.", path, msg)
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

/// (P5-REFINE-7, C-1073) Serialize the direct-conversation decisions of one person and agent
/// (get-or-create, new) for the rest of the transaction.
async fn lock_direct(conn: &mut PgConnection, agent: Uuid, user: Uuid) -> Result<(), ApiError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("direct:{agent}:{user}"))
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    Ok(())
}

fn conversation_limit() -> ApiError {
    ApiError::unprocessable(
        "conversation_limit",
        "You have reached the limit of chats with this agent. Delete some to start a new one.",
    )
}

/// Create a fresh direct conversation: the first is `user:<uid>`, every later one
/// `user:<uid>:<uuid>` (C-1073). Caller holds [`lock_direct`].
async fn create_direct(
    conn: &mut PgConnection,
    agent: &AgentRow,
    user: Uuid,
) -> Result<ConversationRow, ApiError> {
    let n = convs::count_direct(&mut *conn, agent.id, user).await?;
    if n >= aurora_core::limits::DIRECT_CONVERSATIONS_PER_PERSON_AGENT {
        return Err(conversation_limit());
    }
    let first = format!("user:{user}");
    fn new<'a>(agent: &AgentRow, user: Uuid, ext: &'a str) -> NewConversation<'a> {
        NewConversation {
            agent_id: agent.id,
            workspace_id: agent.workspace_id,
            kind: "direct",
            external_ref: ext,
            direct_user_id: Some(user),
            title: None,
            origin_conversation_id: None,
        }
    }
    let (row, created) = convs::get_or_create(&mut *conn, &new(agent, user, &first)).await?;
    if created {
        return Ok(row);
    }
    let ext = format!("user:{user}:{}", Uuid::now_v7());
    Ok(convs::get_or_create(&mut *conn, &new(agent, user, &ext))
        .await?
        .0)
}

/// `POST …/conversations/direct`: the caller's most recently active non-archived direct
/// conversation with the agent, creating the first when there is none (C-1073; never an archived
/// one, UX review 5 chat-1).
pub async fn direct(
    state: &AppState,
    c: &Caller,
    agent_id: Uuid,
) -> Result<Conversation, ApiError> {
    let agent = load_agent(state, c, agent_id).await?;
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    lock_direct(&mut tx, agent_id, c.user_id).await?;
    let row = match convs::latest_direct(&mut *tx, agent_id, c.user_id).await? {
        Some(r) => r,
        None => create_direct(&mut tx, &agent, c.user_id).await?,
    };
    let active = turns::active_turn(&mut *tx, row.id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(Conversation::from_row(row, active))
}

/// `POST …/conversations/direct/new` (C-1073): a fresh direct conversation. `(conversation,
/// created)`: the latest one is returned (not created) while it has no events yet.
pub async fn new_direct(
    state: &AppState,
    c: &Caller,
    agent_id: Uuid,
) -> Result<(Conversation, bool), ApiError> {
    let agent = load_agent(state, c, agent_id).await?;
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    lock_direct(&mut tx, agent_id, c.user_id).await?;
    if let Some(latest) = convs::latest_direct(&mut *tx, agent_id, c.user_id).await?
        && latest.last_seq == 0
    {
        let active = turns::active_turn(&mut *tx, latest.id).await?;
        tx.commit().await.map_err(DbError::from)?;
        return Ok((Conversation::from_row(latest, active), false));
    }
    let row = create_direct(&mut tx, &agent, c.user_id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok((Conversation::from_row(row, None), true))
}

// ---------------------------------------------------------------- titles (C-1073)

/// Longest auto title, in characters (the "…" included).
const AUTO_TITLE_MAX: usize = 60;

/// (C-1073) The auto title of a message: its first line with text (code-fence lines skipped),
/// markup and mentions removed, whitespace collapsed, cut at 60 characters with "…". `None` when
/// nothing is left (an attachment-only message, a fence alone). No model call.
pub fn auto_title(text: &str) -> Option<String> {
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("```") || t.starts_with("~~~") {
            continue;
        }
        let t = t.trim_start_matches(['#', '>', '-', '+', ' ', '\t']);
        let cleaned: String = t
            .chars()
            .filter(|ch| {
                !matches!(ch, '*' | '_' | '`' | '~' | '@')
                    && (!ch.is_control() || ch.is_whitespace())
            })
            .collect();
        let words: Vec<&str> = cleaned.split_whitespace().collect();
        if words.is_empty() {
            continue;
        }
        let one = words.join(" ");
        if one.chars().count() <= AUTO_TITLE_MAX {
            return Some(one);
        }
        let cut: String = one.chars().take(AUTO_TITLE_MAX - 1).collect();
        return Some(format!("{}…", cut.trim_end()));
    }
    None
}

/// The auto title from the conversation's first person message that yields one.
async fn derive_auto_title(conn: &mut PgConnection, id: Uuid) -> Result<Option<String>, ApiError> {
    let texts: Vec<String> = sqlx::query_scalar(
        "SELECT text FROM turns WHERE conversation_id = $1 AND speaker_kind = 'person'
         ORDER BY created_at, id LIMIT 20",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await
    .map_err(DbError::from)?;
    Ok(texts.iter().find_map(|t| auto_title(t)))
}

fn invalid_title(msg: &str) -> ApiError {
    ApiError::unprocessable("invalid_title", "That name can't be used.")
        .with_details(json!({"fields": [{"path": "/title", "message": msg}]}))
}

/// `PATCH /conversations/{id}` (C-1073): rename, or (`None`) go back to the auto title.
pub async fn rename(
    state: &AppState,
    c: &Caller,
    id: Uuid,
    title: Option<&str>,
) -> Result<Conversation, ApiError> {
    let (row, auth) = load_conversation_auth(state, c, id).await?;
    if row.automation_id.is_some() || matches!(row.kind.as_str(), "schedule" | "webhook") {
        return Err(ApiError::conflict(
            "title_managed",
            "This conversation is named after its automation; rename the automation instead.",
        ));
    }
    if row.kind == "direct" {
        if row.direct_user_id != Some(c.user_id) {
            return Err(ApiError::new(
                axum::http::StatusCode::FORBIDDEN,
                "direct_conversation_forbidden",
                "Only the person in a direct conversation can rename it.",
            ));
        }
    } else {
        crate::http::auth::require_own(&auth)?;
    }
    let title = match title.map(str::trim) {
        Some("") => return Err(invalid_title("must not be empty")),
        Some(t) if t.chars().count() > TITLE_MAX => {
            return Err(invalid_title("must be at most 200 characters"));
        }
        Some(t) if t.chars().any(char::is_control) => {
            return Err(invalid_title("must not contain control characters"));
        }
        other => other,
    };
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    if !convs::lock(&mut tx, id).await? {
        return Err(conv_404());
    }
    let updated = match title {
        Some(t) => convs::set_title(&mut tx, id, Some(t), "person").await?,
        None => {
            let auto = derive_auto_title(&mut tx, id).await?;
            convs::set_title(&mut tx, id, auto.as_deref(), "auto").await?
        }
    }
    .ok_or_else(conv_404)?;
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
            action: "conversation.rename",
            target_kind: Some("conversation"),
            target_id: Some(&id.to_string()),
            outcome: Outcome::Success.as_str(),
            source: actor.source,
            ip: actor.ip,
            // Ids only: never the title (C-1073).
            metadata: &json!({"agent_id": row.agent_id, "cleared": title.is_none()}),
        },
    )
    .await?;
    let active = turns::active_turn(&mut *tx, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    let mut out = [Conversation::from_row(updated, active)];
    with_automation(&state.pool, &mut out).await?;
    let [conv] = out;
    Ok(conv)
}

/// (C-1074) Fill `automation.name` / `deleted` from the live schedules and webhooks (one query
/// for the page): a rename shows at once, a missing row means deleted.
pub async fn with_automation(
    pool: &sqlx::PgPool,
    items: &mut [Conversation],
) -> Result<(), ApiError> {
    let (mut sched, mut hooks) = (Vec::new(), Vec::new());
    for a in items.iter().filter_map(|i| i.automation.as_ref()) {
        match a.kind.as_str() {
            "schedule" => sched.push(a.id),
            _ => hooks.push(a.id),
        }
    }
    if sched.is_empty() && hooks.is_empty() {
        return Ok(());
    }
    let names = convs::automation_names(pool, &sched, &hooks).await?;
    for a in items.iter_mut().filter_map(|i| i.automation.as_mut()) {
        match names.iter().find(|(id, _)| *id == a.id) {
            Some((_, name)) => a.name.clone_from(name),
            None => a.deleted = true,
        }
    }
    Ok(())
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
    let mut out = [Conversation::from_row(row, active)];
    with_automation(&state.pool, &mut out).await?;
    let [conv] = out;
    Ok(conv)
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
    let items: Vec<TranscriptEvent> = rows.into_iter().map(TranscriptEvent::from).collect();
    let purged_attachments = purged_attachments_of(state, id, &items).await?;
    Ok(EventsPage {
        items,
        last_seq,
        purged_attachments,
    })
}

/// The purged attachments among those the page's events reference (`attachment` events and
/// `turn.queued.attachments`).
async fn purged_attachments_of(
    state: &AppState,
    conversation: Uuid,
    items: &[TranscriptEvent],
) -> Result<Vec<PurgedAttachment>, ApiError> {
    let mut ids: Vec<Uuid> = Vec::new();
    let mut push = |v: &Value| {
        if let Some(id) = v.as_str().and_then(|s| s.parse::<Uuid>().ok()) {
            ids.push(id);
        }
    };
    for e in items {
        match e.ty.as_str() {
            "attachment" => push(&e.payload["attachment_id"]),
            "turn.queued" => {
                if let Some(list) = e.payload["attachments"].as_array() {
                    list.iter().for_each(&mut push);
                }
            }
            _ => {}
        }
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(
        aurora_db::repo::attachments::purged_among(&state.pool, conversation, &ids)
            .await?
            .into_iter()
            .map(|(id, reason)| PurgedAttachment { id, reason })
            .collect(),
    )
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
    /// (C-725) `seq` of the event this message quotes; validated under the conversation lock.
    pub reply_to_seq: Option<i64>,
}

fn clip(s: &str, max_bytes: usize) -> &str {
    &s[..s.floor_char_boundary(max_bytes)]
}

/// (C-1079) Keep a `turn.queued` payload with `display` inside the event bound: the display
/// body is halved (and marked `truncated`) until the serialized payload fits.
fn fit_display(payload: &mut Value) {
    let max = transcript::MAX_PAYLOAD_BYTES;
    while payload.to_string().len() > max {
        let Some(body) = payload["display"]["body"].as_str() else {
            payload["display"] = Value::Null;
            return;
        };
        if body.is_empty() {
            payload["display"] = Value::Null;
            return;
        }
        let cut = clip(body, body.len() / 2).to_owned();
        payload["display"]["body"] = json!(cut);
        payload["display"]["truncated"] = json!(true);
    }
}

/// `text` bound; empty text is allowed only when files are attached (C-726).
pub fn check_text(text: &str, has_files: bool) -> Result<(), ApiError> {
    if text.trim().is_empty() && !has_files {
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
    create_turn_tx_with(conn, conv, m, None).await
}

/// [`create_turn_tx`] with the `display` object of the `turn.queued` event (P5-REFINE-7,
/// C-1079): what the transcript shows people for a schedule or webhook turn, while the turn's
/// `text` (what the agent receives) is unchanged.
pub async fn create_turn_tx_with(
    conn: &mut PgConnection,
    conv: &ConversationRow,
    m: &NewMessage<'_>,
    display: Option<&Value>,
) -> Result<Created, ApiError> {
    create_turn_capped_with(conn, conv, m, display)
        .await?
        .ok_or_else(|| {
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
    create_turn_capped_with(conn, conv, m, None).await
}

/// [`create_turn_capped`] with the `turn.queued` event's `display` (C-1079).
pub async fn create_turn_capped_with(
    conn: &mut PgConnection,
    conv: &ConversationRow,
    m: &NewMessage<'_>,
    display: Option<&Value>,
) -> Result<Option<Created>, ApiError> {
    if !convs::lock(&mut *conn, conv.id).await? {
        return Err(conv_404());
    }
    // (phase 2, RFE-006) Every door: an effectively frozen agent starts no work (423 `frozen`).
    crate::domains::freeze::service::ensure_agent_active(&mut *conn, conv.agent_id).await?;
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
    // (C-725) Validated and composed under the conversation lock, so a concurrent redaction of
    // the target either happens first (422 `redacted`) or cascades onto this message after.
    let quote = match m.reply_to_seq {
        Some(seq) => Some(super::reply::resolve(&mut *conn, conv.id, conv.agent_id, seq).await?),
        None => None,
    };
    let trace_context = aurora_core::telemetry::current_traceparent();
    let n = NewTurn {
        conversation_id: conv.id,
        agent_id: conv.agent_id,
        door: &m.door,
        speaker_kind: m.speaker_kind,
        speaker_user_id: m.speaker_user_id,
        speaker_label: m.speaker_label,
        text: m.text,
        source_event_id: m.source_event_id,
        reply_to_seq: m.reply_to_seq,
        // (FIX-146, D-232) the turn is a child of the span that inserts it.
        trace_context: trace_context.as_deref(),
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
    if let Some(q) = &quote {
        payload["reply_to"] = q.payload();
    }
    if let Some(d) = display {
        payload["display"] = d.clone();
        fit_display(&mut payload);
    }
    transcript::append_durable_event(&mut *conn, conv.id, Some(row.id), "turn.queued", &payload)
        .await?;
    // (P5-REFINE-7, C-1073) The first person message names an untitled conversation; a title a
    // person chose (or an automation's) is never replaced. Attachment-only messages name nothing.
    if m.speaker_kind == "person"
        && let Some(t) = auto_title(m.text)
    {
        convs::set_auto_title(&mut *conn, conv.id, &t).await?;
    }
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
    // (P5-USAGE-1, C-1001) The ledger grows in the transaction of the fenced transition; a replay returned
    // above and writes nothing. The stored usage was reduced by the trust rule when it was stored.
    aurora_db::repo::usage::record_turn(&mut *conn, turn_id).await?;
    // (P5-REFINE-7, C-1075) A person's Stop leaves a mark, after the partial answer the answer
    // rule already wrote. Endings with their own notice (freeze, deprovision: an error code) and
    // turns nobody asked to stop get none.
    if status == "aborted"
        && row.abort_requested_at.is_some()
        && matches!(row.error_code.as_deref(), None | Some("aborted"))
    {
        transcript::append_durable_event(
            &mut *conn,
            row.conversation_id,
            Some(turn_id),
            "notice",
            &json!({"code": "turn_stopped", "text": "You stopped this answer.", "turn_id": turn_id}),
        )
        .await?;
    }
    let mut payload = json!({"turn_id": turn_id, "status": status});
    if let Some(code) = &row.error_code {
        payload["error_code"] = json!(code);
    }
    if let Some(u) = &row.usage {
        payload["usage"] = u.clone();
    }
    if let Some(class) = ai_key_class(&mut *conn, turn_id).await? {
        payload["ai_key"] = json!(class);
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

/// (C-1100) The class of the key a turn ran on, from the secret the dispatcher recorded in `turns.model_basis.key_id`:
/// `workspace`; `starter` (an organization key the saved placement hands to every new agent of the organization, as
/// C-1049); `organization`. `None` when nothing was recorded or the secret is gone. A class, never a name.
async fn ai_key_class(
    conn: &mut PgConnection,
    turn_id: Uuid,
) -> Result<Option<&'static str>, ApiError> {
    let Some((org, Some(basis))) =
        aurora_db::repo::hosted::model_basis(&mut *conn, turn_id).await?
    else {
        return Ok(None);
    };
    let Some(key) = basis
        .get("key_id")
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok())
    else {
        return Ok(None);
    };
    let Some(is_workspace) =
        aurora_db::repo::hosted::secret_is_workspace_key(&mut *conn, key).await?
    else {
        return Ok(None);
    };
    if is_workspace {
        return Ok(Some("workspace"));
    }
    let starter = match aurora_db::repo::placement::get(&mut *conn).await? {
        Some(p)
            if p.mode == "shared_org" && p.personal_enabled && p.default_org_id == Some(org) =>
        {
            crate::domains::placement::service::starter_ids(&p)
                .0
                .contains(&key)
        }
        _ => false,
    };
    Ok(Some(if starter { "starter" } else { "organization" }))
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
    let mut payload = json!({
        "turn_id": t.id,
        "door": super::dto::door_name(&t.door),
        "speaker_label": t.speaker_label,
        "text": clip(&t.text, EVENT_TEXT_MAX_BYTES),
        "attachments": attachments,
        "promoted_from": f.id,
    });
    if let Some(seq) = t.reply_to_seq {
        // The quote as it stands now (a target redacted meanwhile is a redacted quote).
        payload["reply_to"] =
            super::reply::for_delivery(&mut *conn, t.conversation_id, t.agent_id, seq)
                .await?
                .payload();
    }
    // (P5-SCHED-ONCE, C-1093) A one-time schedule run that queued behind a turn in the person's
    // chat: its run follows the new turn, and the event keeps the schedule's `display`.
    if t.door.get("kind").and_then(Value::as_str) == Some("schedule")
        && let Some(d) =
            crate::domains::schedules::fire::on_followup_promoted(&mut *conn, f.id, t.id).await?
    {
        payload["display"] = d;
        fit_display(&mut payload);
    }
    transcript::append_durable_event(
        &mut *conn,
        t.conversation_id,
        Some(t.id),
        "turn.queued",
        &payload,
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

/// Longest accepted `Idempotency-Key` (C-320).
const IDEMPOTENCY_KEY_MAX: usize = 128;

fn invalid_key() -> ApiError {
    ApiError::unprocessable(
        "idempotency_key_invalid",
        "Idempotency-Key must be 1-128 characters of A-Z a-z 0-9 . _ : ~ - and given once.",
    )
}

/// C-320: validate the raw `Idempotency-Key` header values (none = no deduplication) and build the
/// stored `api:<user_id>:<key>` form (C-321).
fn stored_key(user_id: Uuid, raw: &[Vec<u8>]) -> Result<Option<String>, ApiError> {
    let [v] = raw else {
        return if raw.is_empty() {
            Ok(None)
        } else {
            Err(invalid_key())
        };
    };
    let ok = !v.is_empty()
        && v.len() <= IDEMPOTENCY_KEY_MAX
        && v.iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'~' | b'-'));
    if !ok {
        return Err(invalid_key());
    }
    let key = String::from_utf8_lossy(v);
    Ok(Some(format!("api:{user_id}:{key}")))
}

/// The accepted-message body for a stored row, as it is now (C-323).
async fn accepted_from_row(
    conn: &mut PgConnection,
    row: TurnRow,
) -> Result<MessageAccepted, ApiError> {
    if row.role == "followup" {
        let promoted = if row.status == "promoted" {
            turns::promoted_turn_id(&mut *conn, row.conversation_id, row.id).await?
        } else {
            None
        };
        let mut f = super::dto::Followup::from(row);
        f.promoted_turn_id = promoted;
        Ok(MessageAccepted {
            turn: None,
            followup: Some(f),
        })
    } else {
        Ok(MessageAccepted {
            turn: Some(row.into()),
            followup: None,
        })
    }
}

/// Replay or mismatch for an existing row under a key (C-323, C-324).
async fn replay(
    conn: &mut PgConnection,
    row: TurnRow,
    text: &str,
    attachment_ids: &[Uuid],
    reply_to_seq: Option<i64>,
) -> Result<MessageAccepted, ApiError> {
    let mut stored = row.attachment_ids.clone();
    stored.sort();
    stored.dedup();
    let mut wanted = attachment_ids.to_vec();
    wanted.sort();
    wanted.dedup();
    if row.text != text || stored != wanted || row.reply_to_seq != reply_to_seq {
        let is_followup = row.role == "followup";
        return Err(ApiError::unprocessable(
            "idempotency_key_reused",
            "This Idempotency-Key was already used for a different message.",
        )
        .with_details(json!({
            "turn_id": if is_followup { None } else { Some(row.id) },
            "followup_id": if is_followup { Some(row.id) } else { None },
        })));
    }
    accepted_from_row(conn, row).await
}

/// Create a message. `key_headers` are the raw `Idempotency-Key` header values (C-320). Returns
/// the body and whether it is a replay (`Idempotent-Replayed: true`).
pub async fn post_message(
    state: &AppState,
    c: &Caller,
    conversation_id: Uuid,
    body: &super::dto::SendMessage,
    key_headers: &[Vec<u8>],
) -> Result<(MessageAccepted, bool), ApiError> {
    let (text, attachment_ids) = (body.text.as_str(), body.attachment_ids.as_slice());
    let reply_to_seq = body.reply_to.map(|r| r.seq);
    // C-322: authorization and visibility first, then the header, then the lookup.
    let conv = load_conversation_for_write(state, c, conversation_id).await?;
    let key = stored_key(c.user_id, key_headers)?;
    if let Some(k) = &key {
        let mut conn = state.pool.acquire().await.map_err(DbError::from)?;
        if let Some(row) = turns::by_source_event(&mut conn, conv.id, k).await? {
            let body = replay(&mut conn, row, text, attachment_ids, reply_to_seq).await?;
            return Ok((body, true));
        }
    }
    check_text(text, !attachment_ids.is_empty())?;
    let agent = agents::get(&state.pool, conv.agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    if agent.kind == "hosted" && !state.ops().hosted.enabled {
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
            source_event_id: key.as_deref(),
            attachment_ids: &ids,
            reply_to_seq,
        },
    )
    .await;
    let created = match created {
        Ok(created) => created,
        // A concurrent request with the same key won the race: answer as its replay (C-323).
        Err(e) if key.is_some() && is_key_conflict(&e) => {
            drop(tx); // rolls back
            let mut conn = state.pool.acquire().await.map_err(DbError::from)?;
            let k = key.as_deref().unwrap_or_default();
            let row = turns::by_source_event(&mut conn, conv.id, k)
                .await?
                .ok_or(e)?;
            let body = replay(&mut conn, row, text, attachment_ids, reply_to_seq).await?;
            return Ok((body, true));
        }
        Err(e) => return Err(e),
    };
    tx.commit().await.map_err(DbError::from)?;
    let (Created::Turn(row) | Created::Followup(row)) = created;
    Ok((
        accepted_from_row(
            &mut *state.pool.acquire().await.map_err(DbError::from)?,
            row,
        )
        .await?,
        false,
    ))
}

/// The `turns_source_event_uq` violation `turns::create` surfaces as a 409 `conflict`.
fn is_key_conflict(e: &ApiError) -> bool {
    e.code() == "conflict"
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
    // (phase 2, RFE-006) 423 `frozen` for a frozen agent, ahead of the finished-turn 409.
    crate::domains::freeze::service::ensure_agent_active(&mut tx, t.agent_id).await?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_title_is_the_first_line_in_plain_words() {
        assert_eq!(auto_title(""), None);
        assert_eq!(auto_title("   \n\t\n"), None);
        assert_eq!(
            auto_title("  Deploy   the\tstaging  box  \nsecond line").as_deref(),
            Some("Deploy the staging box")
        );
        assert_eq!(
            auto_title("# **Plan** the `release` for @team").as_deref(),
            Some("Plan the release for team")
        );
        assert_eq!(
            auto_title("> - quoted item").as_deref(),
            Some("quoted item")
        );
        // Emoji are kept whole.
        assert_eq!(
            auto_title("🚀 Launch day 🎉").as_deref(),
            Some("🚀 Launch day 🎉")
        );
        // A fence line is skipped; a fence alone names nothing.
        assert_eq!(
            auto_title("```rust\nfn main() {}\n```").as_deref(),
            Some("fn main() {}")
        );
        assert_eq!(auto_title("```\n```"), None);
        assert_eq!(auto_title("**__``@@**"), None);
    }

    #[test]
    fn auto_title_cuts_at_sixty_characters_with_an_ellipsis() {
        let word = "a".repeat(200);
        let t = auto_title(&word).unwrap();
        assert_eq!(t.chars().count(), 60);
        assert!(t.ends_with('…'));
        assert_eq!(t.trim_end_matches('…'), "a".repeat(59));
        let exact = "b".repeat(60);
        assert_eq!(auto_title(&exact).as_deref(), Some(exact.as_str()));
        let emoji = "😀".repeat(70);
        let t = auto_title(&emoji).unwrap();
        assert_eq!(t.chars().count(), 60);
        // A cut right after a space does not leave it before the ellipsis.
        let spaced = format!("{} tail", "c".repeat(58));
        assert_eq!(auto_title(&spaced).unwrap(), format!("{}…", "c".repeat(58)));
    }

    #[test]
    fn a_display_body_is_halved_until_the_event_fits() {
        let mut p = json!({"text": "x".repeat(40 * 1024),
            "display": {"kind": "webhook", "body": "y".repeat(40 * 1024), "truncated": false}});
        fit_display(&mut p);
        assert!(p.to_string().len() <= transcript::MAX_PAYLOAD_BYTES);
        assert_eq!(p["display"]["truncated"], true);
        assert!(!p["display"]["body"].as_str().unwrap().is_empty());
        let mut small = json!({"text": "x", "display": {"body": "y", "truncated": false}});
        fit_display(&mut small);
        assert_eq!(small["display"]["truncated"], false);
    }
}
