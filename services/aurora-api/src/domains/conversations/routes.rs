use super::bus::BusMsg;
use super::dto::*;
use super::{lifecycle, service};
use crate::app::AppState;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, Pagination};
use aurora_core::ApiError;
use aurora_db::repo::{conversations as convs, turns};
use aurora_db::repo::{transcript, users};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::Stream;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::time::Duration;
use tokio::sync::broadcast::{self, error::RecvError};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

const KINDS: [&str; 8] = [
    "direct", "group", "schedule", "webhook", "watch", "peer", "api", "eval",
];
/// Safety net: re-read from the database even without a notification.
const RESYNC_EVERY: Duration = Duration::from_secs(15);
const STREAM_BATCH: i64 = 500;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_conversations, create_api_conversation))
        .routes(routes!(direct_conversation))
        .routes(routes!(new_direct_conversation))
        .routes(routes!(list_conversation_folders))
        .routes(routes!(get_conversation))
        .routes(routes!(update_conversation))
        .routes(routes!(list_events))
        .routes(routes!(stream))
        .routes(routes!(post_message))
        .routes(routes!(abort_turn))
        .routes(routes!(send_now))
        .routes(routes!(remind_conversation))
        .routes(routes!(list_followups))
        .routes(routes!(steer_followup))
        .routes(routes!(cancel_followup))
        .routes(routes!(edit_followup))
        .routes(routes!(continue_conversation))
        .routes(routes!(archive_conversation))
        .routes(routes!(unarchive_conversation))
        .routes(routes!(delete_conversation))
        .routes(routes!(redact_event))
        .routes(routes!(redact_turn))
        .routes(routes!(export_conversation))
        .routes(routes!(get_retention, put_retention))
        .merge(super::search::router())
}

/// Archive a conversation (hidden from default lists; a person message unarchives).
#[utoipa::path(post, path = "/conversations/{id}/archive", operation_id = "archiveConversation", tag = "conversations",
    params(("id" = Uuid, Path, description = "Conversation id")),
    responses((status = 200, body = Conversation), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn archive_conversation(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Conversation>, ApiError> {
    Ok(Json(lifecycle::set_archived(&s, &c, id, true).await?))
}

/// Unarchive a conversation.
#[utoipa::path(post, path = "/conversations/{id}/unarchive", operation_id = "unarchiveConversation", tag = "conversations",
    params(("id" = Uuid, Path, description = "Conversation id")),
    responses((status = 200, body = Conversation), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn unarchive_conversation(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Conversation>, ApiError> {
    Ok(Json(lifecycle::set_archived(&s, &c, id, false).await?))
}

/// Permanently delete a conversation with its transcript, attachments and harness session files.
#[utoipa::path(delete, path = "/conversations/{id}", operation_id = "deleteConversation", tag = "conversations",
    params(("id" = Uuid, Path, description = "Conversation id")),
    responses((status = 204, description = "Deleted"), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope),
        (status = 409, description = "conversation_active", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_conversation(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    lifecycle::delete(&s, &c, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Redact one transcript event in place (irreversible, audited).
#[utoipa::path(post, path = "/conversations/{id}/events/{seq}/redact", operation_id = "redactConversationEvent", tag = "conversations",
    params(("id" = Uuid, Path, description = "Conversation id"), ("seq" = i64, Path, description = "Event seq")),
    request_body = RedactRequest,
    responses((status = 200, body = RedactResult), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn redact_event(
    c: Caller,
    State(s): State<AppState>,
    Path((id, seq)): Path<(Uuid, i64)>,
    Json(req): Json<RedactRequest>,
) -> Result<Json<RedactResult>, ApiError> {
    Ok(Json(
        lifecycle::redact(&s, &c, id, lifecycle::Target::Event(seq), &req).await?,
    ))
}

/// Redact every event of a turn (irreversible, audited).
#[utoipa::path(post, path = "/turns/{id}/redact", operation_id = "redactTurn", tag = "conversations",
    params(("id" = Uuid, Path, description = "Turn id")),
    request_body = RedactRequest,
    responses((status = 200, body = RedactResult), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn redact_turn(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<RedactRequest>,
) -> Result<Json<RedactResult>, ApiError> {
    let t = service::load_turn(&s, &c, id).await?;
    Ok(Json(
        lifecycle::redact(&s, &c, t.conversation_id, lifecycle::Target::Turn(id), &req).await?,
    ))
}

/// Export a conversation as `aurora.conversation.v1` JSON (or a zip with the attachment files).
#[utoipa::path(get, path = "/conversations/{id}/export", operation_id = "exportConversation", tag = "conversations",
    params(("id" = Uuid, Path, description = "Conversation id"), ExportQuery),
    responses((status = 200, description = "application/json, or application/zip with attachments=zip"),
        (status = 400, body = ErrorEnvelope), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope),
        (status = 422, description = "export_too_large", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn export_conversation(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let zip = match q.attachments.as_deref() {
        None | Some("none") => false,
        Some("zip") => true,
        Some(_) => {
            return Err(ApiError::invalid_field(
                "Invalid request.",
                "/attachments",
                "must be `none` or `zip`",
            ));
        }
    };
    let out = lifecycle::export(&s, &c, id, q.after_seq, zip).await?;
    Ok((
        [
            (
                axum::http::header::CONTENT_TYPE,
                out.content_type.to_owned(),
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", out.filename),
            ),
            (axum::http::header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        out.bytes,
    )
        .into_response())
}

/// The organization's conversation retention window (org `owner`/`admin`).
#[utoipa::path(get, path = "/orgs/{org}/retention", operation_id = "getOrgRetention", tag = "orgs",
    params(("org" = Uuid, Path, description = "Organization id")),
    responses((status = 200, body = Retention), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_retention(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
) -> Result<Json<Retention>, ApiError> {
    Ok(Json(lifecycle::get_retention(&s, &c, org).await?))
}

/// Set or clear the conversation retention window (org `owner`/`admin`, audited).
#[utoipa::path(put, path = "/orgs/{org}/retention", operation_id = "putOrgRetention", tag = "orgs",
    params(("org" = Uuid, Path, description = "Organization id")),
    request_body = PutRetention,
    responses((status = 200, body = Retention), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 422, description = "retention_invalid | retention_above_instance_maximum", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn put_retention(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Json(b): Json<PutRetention>,
) -> Result<Json<Retention>, ApiError> {
    Ok(Json(lifecycle::put_retention(&s, &c, org, b).await?))
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
    // Authorise before parsing the query (FIX-181 F7, C-1041).
    let (agent_row, read) = service::load_agent_read_auth(&s, &c, agent).await?;
    if let Some(k) = q.kind.as_deref()
        && !KINDS.contains(&k)
    {
        return Err(ApiError::invalid_field(
            "Invalid kind.",
            "/kind",
            "unknown conversation kind",
        ));
    }
    // (P5-REFINE-7, C-1074) `automation=all|none|only`, `automation_id` only with `only`.
    let automation = match (q.automation.as_deref(), q.automation_id) {
        (None | Some("all"), None) => convs::AutomationFilter::All,
        (Some("none"), None) => convs::AutomationFilter::None,
        (Some("only"), id) => convs::AutomationFilter::Only(id),
        (Some("all" | "none") | None, Some(_)) => {
            return Err(ApiError::invalid_field(
                "Invalid request.",
                "/automation_id",
                "only with automation=only",
            ));
        }
        (Some(_), _) => {
            return Err(ApiError::invalid_field(
                "Invalid request.",
                "/automation",
                "one of all, none, only",
            ));
        }
    };
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    // RFE-009: owners see every direct conversation of the agent; others only their own. An
    // emergency reader reads what an owner reads (C-1165, D-478(b)).
    let owns = c
        .workspace_read(&s, agent_row.workspace_id)
        .await?
        .can_own();
    let rows = convs::list_for_agent(
        &s.pool,
        agent,
        &convs::ListFilter {
            kind: q.kind.as_deref(),
            viewer: c.user_id,
            viewer_owns: owns,
            archived: q.archived.unwrap_or(false),
            automation,
        },
        p.after_key(),
        p.fetch(),
    )
    .await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.last_activity_at, r.id));
    let ids: Vec<Uuid> = page.items.iter().map(|r| r.id).collect();
    let mut active = turns::active_turns(&s.pool, &ids).await?;
    let mut items = Vec::with_capacity(page.items.len());
    for r in page.items {
        let a = active
            .iter()
            .position(|t| t.conversation_id == r.id)
            .map(|i| active.swap_remove(i));
        let other_direct = if r.kind == "direct" {
            r.direct_user_id.filter(|u| *u != c.user_id)
        } else {
            None
        };
        let mut conv = Conversation::from_row(r, a);
        if let Some(u) = other_direct {
            let display_name = users::find_by_id(&s.pool, u)
                .await?
                .map(|u| u.display_name)
                .unwrap_or_default();
            conv.direct_user = Some(DirectUser {
                id: u,
                display_name,
            });
        }
        items.push(conv);
    }
    service::with_automation(&s.pool, &mut items).await?;
    // (FIX-236, review LOW-12) The list carries conversation content (each row's active turn's
    // text, and for direct rows who else takes part): the emergency reader's views for the
    // listed conversations are recorded before the body, like the search and the feed — a
    // failed write refuses the read with 503.
    if let aurora_db::authz::ReadAuth::Emergency(g) = &read {
        let actor = crate::audit::AuditActor::from_caller(&c);
        for id in &ids {
            service::record_emergency_view_id(&s, &actor, *id, g).await?;
        }
    }
    Ok(Json(Page {
        items,
        next_cursor: page.next_cursor,
    }))
}

/// The caller's most recently active non-archived direct conversation with the agent, created
/// when there is none (C-1073).
#[utoipa::path(post, path = "/agents/{agent}/conversations/direct", operation_id = "getOrCreateDirectConversation", tag = "conversations",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = Conversation), (status = 404, body = ErrorEnvelope),
        (status = 422, description = "conversation_limit", body = ErrorEnvelope)),
    security(("session" = [])))]
async fn direct_conversation(
    c: crate::http::auth::SessionCaller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<Conversation>, ApiError> {
    Ok(Json(service::direct(&s, &c, agent).await?))
}

/// A fresh direct conversation with empty history (C-1073): 200 with the latest one while it is
/// still empty, 201 with a new one otherwise.
#[utoipa::path(post, path = "/agents/{agent}/conversations/direct/new", operation_id = "createDirectConversation", tag = "conversations",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, description = "The latest, still empty", body = Conversation),
        (status = 201, description = "Created", body = Conversation),
        (status = 404, body = ErrorEnvelope),
        (status = 422, description = "conversation_limit", body = ErrorEnvelope)),
    security(("session" = [])))]
async fn new_direct_conversation(
    c: crate::http::auth::SessionCaller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Response, ApiError> {
    let (conv, created) = service::new_direct(&s, &c, agent).await?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(conv)).into_response())
}

/// Rename a conversation, or (`title: null`) go back to the automatic title (C-1073).
#[utoipa::path(patch, path = "/conversations/{id}", operation_id = "updateConversation", tag = "conversations",
    params(("id" = Uuid, Path, description = "Conversation id")),
    request_body = UpdateConversation,
    responses((status = 200, body = Conversation), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope),
        (status = 409, description = "title_managed", body = ErrorEnvelope),
        (status = 422, description = "invalid_title", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn update_conversation(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(b): Json<UpdateConversation>,
) -> Result<Json<Conversation>, ApiError> {
    Ok(Json(service::rename(&s, &c, id, b.title.as_deref()).await?))
}

/// The agent's automation folders: one per schedule or webhook that has run threads (C-1074).
#[utoipa::path(get, path = "/agents/{agent}/conversation-folders", operation_id = "listConversationFolders", tag = "conversations",
    params(("agent" = Uuid, Path, description = "Agent id"), FoldersQuery),
    responses((status = 200, body = Page<ConversationFolder>), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_conversation_folders(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Query(q): Query<FoldersQuery>,
) -> Result<Json<Page<ConversationFolder>>, ApiError> {
    service::load_agent_read(&s, &c, agent).await?;
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    let rows = convs::list_folders(&s.pool, agent, p.after_key(), p.fetch()).await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.last_activity_at, r.automation_id));
    let ids_of = |k: &str| -> Vec<Uuid> {
        page.items
            .iter()
            .filter(|f| f.automation_kind == k)
            .map(|f| f.automation_id)
            .collect()
    };
    let runs = aurora_db::repo::schedules::latest_runs(&s.pool, &ids_of("schedule")).await?;
    let deliveries =
        aurora_db::repo::webhooks::latest_deliveries(&s.pool, &ids_of("webhook")).await?;
    use crate::domains::schedules::results;
    Ok(Json(page.map(|f| {
        let last_result = if f.automation_kind == "schedule" {
            runs.iter()
                .find(|r| r.schedule_id == f.automation_id)
                .map(|r| {
                    results::run_result(
                        &r.outcome,
                        r.turn_status.as_deref(),
                        r.turn_error_code.as_deref(),
                    )
                    .0
                    .to_owned()
                })
        } else {
            deliveries
                .iter()
                .find(|d| d.webhook_id == f.automation_id)
                .map(|d| {
                    results::delivery_result(
                        &d.status,
                        d.reason.as_deref(),
                        d.turn_status.as_deref(),
                        d.turn_error_code.as_deref(),
                    )
                    .to_owned()
                })
        };
        ConversationFolder {
            kind: f.automation_kind,
            id: f.automation_id,
            name: f.name.unwrap_or_default(),
            deleted: f.deleted,
            thread_count: f.thread_count,
            last_activity_at: f.last_activity_at,
            last_result,
        }
    })))
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
    params(("id" = Uuid, Path, description = "Conversation id"),
        ("Idempotency-Key" = Option<String>, Header,
            description = "1-128 characters of [A-Za-z0-9._:~-]. A retry with the same key returns the original result with `Idempotent-Replayed: true` and creates nothing.")),
    request_body = SendMessage,
    responses((status = 202, body = MessageAccepted), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 413, body = ErrorEnvelope),
        (status = 422, description = "hosted_disabled, idempotency_key_invalid, idempotency_key_reused (details.turn_id | details.followup_id), reply_target_invalid (details.reason: not_found | not_replyable | redacted)", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn post_message(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(b): Json<SendMessage>,
) -> Result<(StatusCode, HeaderMap, Json<MessageAccepted>), ApiError> {
    let key_headers: Vec<Vec<u8>> = headers
        .get_all("idempotency-key")
        .iter()
        .map(|v| v.as_bytes().to_vec())
        .collect();
    let (out, replayed) = service::post_message(&s, &c, id, &b, &key_headers).await?;
    let mut h = HeaderMap::new();
    if replayed {
        h.insert("idempotent-replayed", HeaderValue::from_static("true"));
    }
    Ok((StatusCode::ACCEPTED, h, Json(out)))
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

/// The follow-ups waiting in a conversation (sent while a turn runs), oldest first, each with the
/// controls the caller may use now.
#[utoipa::path(get, path = "/conversations/{id}/followups", operation_id = "listFollowups", tag = "conversations",
    params(("id" = Uuid, Path, description = "Conversation id")),
    responses((status = 200, body = FollowupList), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_followups(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<FollowupList>, ApiError> {
    Ok(Json(super::followups::list(&s, &c, id).await?))
}

/// Ask for a waiting follow-up to be steered into the running turn at its next step.
#[utoipa::path(post, path = "/followups/{id}/steer", operation_id = "steerFollowup", tag = "conversations",
    params(("id" = Uuid, Path, description = "Follow-up id")),
    responses((status = 202, body = Followup), (status = 404, body = ErrorEnvelope),
        (status = 409, description = "followup_not_waiting, steer_unsupported", body = ErrorEnvelope),
        (status = 423, description = "frozen", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn steer_followup(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<Followup>), ApiError> {
    let out = super::followups::steer(&s, &c, id).await?;
    Ok((StatusCode::ACCEPTED, Json(out.followup)))
}

/// Withdraw a waiting follow-up; it is never sent.
#[utoipa::path(post, path = "/followups/{id}/cancel", operation_id = "cancelFollowup", tag = "conversations",
    params(("id" = Uuid, Path, description = "Follow-up id")),
    responses((status = 202, body = Followup), (status = 403, description = "followup_forbidden", body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope),
        (status = 409, description = "followup_not_waiting", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn cancel_followup(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<Followup>), ApiError> {
    Ok((
        StatusCode::ACCEPTED,
        Json(super::followups::cancel(&s, &c, id).await?),
    ))
}

/// Replace the text of a waiting follow-up. Only the person who wrote it.
#[utoipa::path(patch, path = "/followups/{id}", operation_id = "editFollowup", tag = "conversations",
    params(("id" = Uuid, Path, description = "Follow-up id")),
    request_body = EditFollowup,
    responses((status = 200, body = Followup), (status = 400, body = ErrorEnvelope),
        (status = 403, description = "followup_forbidden", body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope),
        (status = 409, description = "followup_not_waiting", body = ErrorEnvelope),
        (status = 413, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn edit_followup(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(b): Json<EditFollowup>,
) -> Result<Json<Followup>, ApiError> {
    Ok(Json(super::followups::edit(&s, &c, id, &b.text).await?))
}

/// Ask the harness to move a blocking tool to the background.
#[utoipa::path(post, path = "/turns/{id}/send-now", operation_id = "sendNowTurn", tag = "conversations",
    params(("id" = Uuid, Path, description = "Turn id")),
    responses((status = 202, body = Turn), (status = 404, body = ErrorEnvelope),
        (status = 409, description = "turn_finished, send_now_unsupported (the harness lacks background_tool or steers)", body = ErrorEnvelope)),
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

/// (FIX-253, D-490) The "Remind it in 15 minutes" action on the `no_wakeup` notice: one one-time
/// schedule 15 minutes ahead in this conversation, attributed to the caller; a second press
/// returns the already-set time.
#[utoipa::path(post, path = "/conversations/{id}/reminder", operation_id = "remindConversation", tag = "conversations",
    params(("id" = Uuid, Path, description = "Conversation id")),
    request_body = ReminderRequest,
    responses((status = 200, body = ReminderSet), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope),
        (status = 409, description = "reminder_not_offered", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn remind_conversation(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(b): Json<ReminderRequest>,
) -> Result<Json<ReminderSet>, ApiError> {
    Ok(Json(
        service::remind_in_15_minutes(&s, &c, id, b.turn_id).await?,
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
    user: Uuid,
    last: i64,
    buf: VecDeque<Event>,
    tick: tokio::time::Interval,
    dirty: bool,
    /// (P4-GRP-1) `access` pings: re-authorize when the caller's (or their org's) access changed.
    access: broadcast::Receiver<crate::http::auth::AccessPing>,
    /// Set once the final `access.changed` event is queued: the stream ends after it.
    closing: bool,
    /// (Verify LOW-3) The caller's emergency-view dedup window: `Unknown` until first needed,
    /// `NotReader` never gates, `Reader` gates content frames until the instant.
    window: WindowState,
}

/// (Verify LOW-3) See [`Stream_.window`].
enum WindowState {
    Unknown,
    NotReader,
    Reader(tokio::time::Instant),
}

impl Stream_ {
    /// The viewer lost the right to see this conversation: queue the final `access.changed`
    /// event (the client reconnects and gets 403/404) and end the stream after it.
    fn close_access_changed(&mut self) {
        if let Ok(e) = Event::default()
            .event("access.changed")
            .json_data(serde_json::json!({}))
        {
            self.buf.push_back(e);
        }
        self.closing = true;
    }

    /// (Verify LOW-3) The dedup-hour gate before any content frame: inside the caller's window
    /// content flows (the fast path touches nothing); once the window has rolled over, the
    /// fresh `emergency_view` row is written FIRST, and a failed write ends the stream — the
    /// client's reconnect is then refused until the audit writes again, so content never flows
    /// unrecorded. Returns false when the stream must close.
    async fn ensure_recorded(&mut self) -> bool {
        if let WindowState::Reader(until) = self.window
            && tokio::time::Instant::now() < until
        {
            return true;
        }
        match service::emergency_view_window(&self.s, self.user, self.conv).await {
            Ok(service::EmergencyViewWindow::NotReader) => {
                self.window = WindowState::NotReader;
                true
            }
            Ok(service::EmergencyViewWindow::Open(until)) => {
                self.set_window(until);
                true
            }
            Ok(service::EmergencyViewWindow::Rolled) => {
                if let Err(e) = service::refresh_emergency_view(&self.s, self.user, self.conv).await
                {
                    tracing::warn!(error = %e.code(), "conversation stream emergency view gate record failed");
                    self.close_access_changed();
                    return false;
                }
                match service::emergency_view_window(&self.s, self.user, self.conv).await {
                    Ok(service::EmergencyViewWindow::Open(until)) => {
                        self.set_window(until);
                        true
                    }
                    // The record could not be confirmed: refuse the content.
                    _ => {
                        self.close_access_changed();
                        false
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "conversation stream emergency view gate failed");
                self.close_access_changed();
                false
            }
        }
    }

    fn set_window(&mut self, until: chrono::DateTime<chrono::Utc>) {
        let dur = (until - chrono::Utc::now())
            .to_std()
            .unwrap_or(std::time::Duration::ZERO);
        self.window = WindowState::Reader(tokio::time::Instant::now() + dur);
    }

    /// Read durable events after `last` from the database until caught up.
    async fn reload(&mut self) {
        self.dirty = false;
        // (Verify LOW-3) The replayed transcript is content: the gate comes first.
        if !self.ensure_recorded().await {
            return;
        }
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
    service::read_conversation(&s, &c, id).await?;
    let from_header = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<i64>().ok());
    let last = from_header.or(q.after_seq).unwrap_or(0).max(0);
    // Subscribe before the first read so nothing committed in between is missed.
    let rx = s.conversation_bus.subscribe(&s.pool).await?;
    let keepalive = s.sse_keepalive;
    let gone = crate::http::auth::revoked(s.clone(), c.clone());
    let mut tick = tokio::time::interval(s.stream_recheck.min(RESYNC_EVERY));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let access = s.session_cache.subscribe_access();
    let st = Stream_ {
        access,
        closing: false,
        rx,
        s,
        conv: id,
        user: c.user_id,
        last,
        buf: VecDeque::new(),
        tick,
        dirty: true,
        window: WindowState::Unknown,
    };
    let events = futures::stream::unfold(st, |mut st| async move {
        loop {
            if let Some(ev) = st.buf.pop_front() {
                return Some((Ok(ev), st));
            }
            if st.closing {
                return None;
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
                            // (Verify LOW-3) Deltas are content: the dedup gate comes first.
                            if !st.ensure_recorded().await {
                                continue;
                            }
                            let payload = match kind.as_str() {
                                "activity" => serde_json::json!({"turn_id": turn, "line": text}),
                                "event.redacted" => {
                                    serde_json::json!({"seq": text.parse::<i64>().unwrap_or(0)})
                                }
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
                ping = st.access.recv() => {
                    // (P4-GRP-1, D-130) A group/binding/mapping change concerning this viewer:
                    // re-authorize now. A still-authorized stream is untouched.
                    let relevant = match ping {
                        Ok(p) => p.concerns(st.user),
                        Err(broadcast::error::RecvError::Lagged(_)) => true,
                        Err(broadcast::error::RecvError::Closed) => false,
                    };
                    if relevant
                        && matches!(
                            aurora_db::authz::can_see_conversation_id_emergency(
                                &st.s.pool,
                                st.user,
                                st.conv
                            )
                            .await,
                            Ok(false)
                        )
                    {
                        st.close_access_changed();
                    }
                }
                _ = st.tick.tick() => {
                    // RFE-009: a viewer who lost visibility (left the workspace, lost
                    // ownership, or whose emergency grant ended) stops receiving the stream at
                    // the next resync.
                    if matches!(
                        aurora_db::authz::can_see_conversation_id_emergency(
                            &st.s.pool,
                            st.user,
                            st.conv
                        )
                        .await,
                        Ok(false)
                    ) {
                        st.close_access_changed();
                        continue;
                    }
                    // (FIX-236, review MEDIUM-4) A live stream keeps recording: when the hour
                    // window has rolled over, the tick writes a fresh `emergency_view` row (the
                    // dedup makes it a no-op otherwise). A failed write ends the stream — the
                    // client's reconnect is refused until the audit writes again, so content
                    // never flows unrecorded.
                    if let Err(e) =
                        service::refresh_emergency_view(&st.s, st.user, st.conv).await
                    {
                        tracing::warn!(error = %e.code(), "conversation stream emergency view re-record failed");
                        st.close_access_changed();
                        continue;
                    }
                    st.dirty = true;
                }
            }
        }
    });
    let events = futures::StreamExt::take_until(events, gone);
    Ok(Sse::new(events).keep_alive(KeepAlive::new().interval(keepalive)))
}
