use super::bus::BusMsg;
use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, Pagination};
use aurora_core::{ApiError, FieldError};
use aurora_db::repo::transcript;
use aurora_db::repo::{conversations as convs, turns};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::Stream;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::time::Duration;
use tokio::sync::broadcast::{self, error::RecvError};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

const KINDS: [&str; 7] = [
    "direct", "group", "schedule", "webhook", "watch", "peer", "api",
];
/// Safety net: re-read from the database even without a notification.
const RESYNC_EVERY: Duration = Duration::from_secs(15);
const STREAM_BATCH: i64 = 500;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_conversations, create_api_conversation))
        .routes(routes!(direct_conversation))
        .routes(routes!(get_conversation))
        .routes(routes!(list_events))
        .routes(routes!(stream))
        .routes(routes!(post_message))
        .routes(routes!(abort_turn))
        .routes(routes!(send_now))
        .routes(routes!(continue_conversation))
}

/// Conversations of an agent, most recently active first.
#[utoipa::path(get, path = "/agents/{agent}/conversations", operation_id = "listConversations", tag = "conversations",
    params(("agent" = Uuid, Path, description = "Agent id"), ListQuery),
    responses((status = 200, body = Page<Conversation>), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_conversations(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Page<Conversation>>, ApiError> {
    if let Some(k) = q.kind.as_deref()
        && !KINDS.contains(&k)
    {
        return Err(ApiError::invalid_request(
            "Invalid kind.",
            vec![FieldError::new("/kind", "unknown conversation kind")],
        ));
    }
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    service::load_agent(&s, &c, agent).await?;
    let rows = convs::list_for_agent(
        &s.pool,
        agent,
        q.kind.as_deref(),
        c.user_id,
        p.after.map(|a| (a.t, a.id)),
        p.fetch(),
    )
    .await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.last_activity_at, r.id));
    let ids: Vec<Uuid> = page.items.iter().map(|r| r.id).collect();
    let mut active = turns::active_turns(&s.pool, &ids).await?;
    let items = page
        .items
        .into_iter()
        .map(|r| {
            let a = active
                .iter()
                .position(|t| t.conversation_id == r.id)
                .map(|i| active.swap_remove(i));
            Conversation::from_row(r, a)
        })
        .collect();
    Ok(Json(Page {
        items,
        next_cursor: page.next_cursor,
    }))
}

/// Get-or-create the caller's direct conversation with the agent.
#[utoipa::path(post, path = "/agents/{agent}/conversations/direct", operation_id = "getOrCreateDirectConversation", tag = "conversations",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = Conversation), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn direct_conversation(
    c: crate::http::auth::SessionCaller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<Conversation>, ApiError> {
    Ok(Json(service::direct(&s, &c, agent).await?))
}

/// Get-or-create an `api` conversation by caller-supplied reference (API key).
#[utoipa::path(post, path = "/agents/{agent}/conversations", operation_id = "createApiConversation", tag = "conversations",
    params(("agent" = Uuid, Path, description = "Agent id")),
    request_body = CreateApiConversation,
    responses((status = 200, description = "Existing", body = Conversation),
        (status = 201, description = "Created", body = Conversation),
        (status = 400, body = ErrorEnvelope), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("api_key" = [])))]
async fn create_api_conversation(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Json(b): Json<CreateApiConversation>,
) -> Result<Response, ApiError> {
    let (conv, created) =
        service::api_conversation(&s, &c, agent, &b.external_ref, b.title.as_deref()).await?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(conv)).into_response())
}

/// One conversation.
#[utoipa::path(get, path = "/conversations/{id}", operation_id = "getConversation", tag = "conversations",
    params(("id" = Uuid, Path, description = "Conversation id")),
    responses((status = 200, body = Conversation), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_conversation(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Conversation>, ApiError> {
    Ok(Json(service::get(&s, &c, id).await?))
}

/// Durable transcript events after `after_seq` (ephemeral events are never returned).
#[utoipa::path(get, path = "/conversations/{id}/events", operation_id = "listConversationEvents", tag = "conversations",
    params(("id" = Uuid, Path, description = "Conversation id"), EventsQuery),
    responses((status = 200, body = EventsPage), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_events(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<EventsQuery>,
) -> Result<Json<EventsPage>, ApiError> {
    Ok(Json(
        service::events(&s, &c, id, q.after_seq, q.limit).await?,
    ))
}

/// Send a message: starts a turn, or becomes a follow-up when a turn is active.
#[utoipa::path(post, path = "/conversations/{id}/messages", operation_id = "postConversationMessage", tag = "conversations",
    params(("id" = Uuid, Path, description = "Conversation id")),
    request_body = SendMessage,
    responses((status = 202, body = MessageAccepted), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 413, body = ErrorEnvelope),
        (status = 422, description = "hosted_disabled", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn post_message(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(b): Json<SendMessage>,
) -> Result<(StatusCode, Json<MessageAccepted>), ApiError> {
    let out = service::post_message(&s, &c, id, &b.text, &b.attachment_ids).await?;
    Ok((StatusCode::ACCEPTED, Json(out)))
}

/// Abort a turn (queued turns end immediately).
#[utoipa::path(post, path = "/turns/{id}/abort", operation_id = "abortTurn", tag = "conversations",
    params(("id" = Uuid, Path, description = "Turn id")),
    responses((status = 202, body = Turn), (status = 404, body = ErrorEnvelope),
        (status = 409, description = "turn_finished", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn abort_turn(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<Turn>), ApiError> {
    Ok((
        StatusCode::ACCEPTED,
        Json(service::abort(&s, &c, id).await?),
    ))
}

/// Ask the harness to move a blocking tool to the background.
#[utoipa::path(post, path = "/turns/{id}/send-now", operation_id = "sendNowTurn", tag = "conversations",
    params(("id" = Uuid, Path, description = "Turn id")),
    responses((status = 202, body = Turn), (status = 404, body = ErrorEnvelope),
        (status = 409, description = "turn_finished", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn send_now(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<Turn>), ApiError> {
    Ok((
        StatusCode::ACCEPTED,
        Json(service::send_now(&s, &c, id).await?),
    ))
}

/// Reset the app-turn streak so bot/agent turns flow again (C-27).
#[utoipa::path(post, path = "/conversations/{id}/continue", operation_id = "continueConversation", tag = "conversations",
    params(("id" = Uuid, Path, description = "Conversation id")),
    responses((status = 200, body = Conversation), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn continue_conversation(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Conversation>, ApiError> {
    Ok(Json(service::continue_conversation(&s, &c, id).await?))
}

struct Stream_ {
    rx: broadcast::Receiver<BusMsg>,
    s: AppState,
    conv: Uuid,
    last: i64,
    buf: VecDeque<Event>,
    tick: tokio::time::Interval,
    dirty: bool,
}

impl Stream_ {
    /// Read durable events after `last` from the database until caught up.
    async fn reload(&mut self) {
        self.dirty = false;
        loop {
            let rows = match transcript::list_after(
                &self.s.pool,
                self.conv,
                self.last,
                STREAM_BATCH,
            )
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "stream reload failed");
                    return;
                }
            };
            let n = rows.len() as i64;
            for r in rows {
                self.last = r.seq;
                let seq = r.seq;
                let ev: TranscriptEvent = r.into();
                if let Ok(e) = Event::default()
                    .event(ev.ty.clone())
                    .id(seq.to_string())
                    .json_data(&ev)
                {
                    self.buf.push_back(e);
                }
            }
            if n < STREAM_BATCH {
                return;
            }
        }
    }
}

/// Server-sent events: `event: <type>`, `id: <seq>` (durable only), `data: <TranscriptEvent>`.
/// Replays durable events after `after_seq` / `Last-Event-ID`, then tails; ephemeral deltas carry
/// no id.
#[utoipa::path(get, path = "/conversations/{id}/stream", operation_id = "streamConversation", tag = "conversations",
    params(("id" = Uuid, Path, description = "Conversation id"), StreamQuery),
    responses((status = 200, content_type = "text/event-stream", body = TranscriptEvent),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn stream(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<StreamQuery>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    service::load_conversation(&s, &c, id).await?;
    let from_header = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<i64>().ok());
    let last = from_header.or(q.after_seq).unwrap_or(0).max(0);
    // Subscribe before the first read so nothing committed in between is missed.
    let rx = s.conversation_bus.subscribe(&s.pool).await?;
    let keepalive = s.sse_keepalive;
    let gone = crate::http::auth::revoked(s.clone(), c.clone());
    let mut tick = tokio::time::interval(RESYNC_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let st = Stream_ {
        rx,
        s,
        conv: id,
        last,
        buf: VecDeque::new(),
        tick,
        dirty: true,
    };
    let events = futures::stream::unfold(st, |mut st| async move {
        loop {
            if let Some(ev) = st.buf.pop_front() {
                return Some((Ok(ev), st));
            }
            if st.dirty {
                st.reload().await;
                continue;
            }
            tokio::select! {
                msg = st.rx.recv() => match msg {
                    Ok(BusMsg::Seq { conversation, seq }) => {
                        if conversation == st.conv && seq > st.last {
                            st.dirty = true;
                        }
                    }
                    Ok(BusMsg::Delta { conversation, turn, kind, message_id, reply_to, text }) => {
                        if conversation == st.conv {
                            let payload = match kind.as_str() {
                                "activity" => serde_json::json!({"turn_id": turn, "line": text}),
                                _ => {
                                    let mut p = serde_json::json!({"turn_id": turn, "delta": text});
                                    if let Some(m) = message_id {
                                        p["message_id"] = serde_json::Value::String(m);
                                    }
                                    if let Some(rt) = reply_to {
                                        p["reply_to"] = serde_json::Value::String(rt);
                                    }
                                    p
                                }
                            };
                            let ev = TranscriptEvent {
                                seq: None,
                                ty: kind.clone(),
                                ts: chrono::Utc::now(),
                                payload,
                            };
                            if let Ok(e) = Event::default().event(kind).json_data(&ev) {
                                st.buf.push_back(e);
                            }
                        }
                    }
                    Ok(BusMsg::Resync) | Err(RecvError::Lagged(_)) => st.dirty = true,
                    Err(RecvError::Closed) => return None,
                },
                _ = st.tick.tick() => st.dirty = true,
            }
        }
    });
    let events = futures::StreamExt::take_until(events, gone);
    Ok(Sse::new(events).keep_alive(KeepAlive::new().interval(keepalive)))
}
