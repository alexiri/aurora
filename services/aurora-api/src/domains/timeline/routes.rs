use super::dto::{StreamQuery, TimelinePage, TimelineQuery, TimelineStreamEvents};
use super::feed::{self, AgentCtx, Cursor, Feed, Filter, Viewer};
use super::stream::Stream;
use crate::app::AppState;
use crate::http::auth::{Caller, SessionCaller};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use aurora_core::ApiError;
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use std::convert::Infallible;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(timeline))
        .routes(routes!(timeline_stream))
}

/// The agent and the caller's read authority to its workspace (404 as every agent route): own
/// access, or an active emergency grant (C-1165).
async fn resolve(s: &AppState, c: &Caller, agent: Uuid) -> Result<(Viewer, AgentCtx), ApiError> {
    let a = crate::domains::conversations::service::load_agent_read(s, c, agent).await?;
    let auth = c.workspace_read(s, a.workspace_id).await?;
    let harness = a
        .harness
        .clone()
        .unwrap_or_else(|| s.ops().hosted.default_harness.clone());
    Ok((
        Viewer {
            user: c.user_id,
            actor: crate::audit::AuditActor::from_caller(c),
            auth,
        },
        AgentCtx {
            id: a.id,
            name: a.name,
            workspace_id: a.workspace_id,
            harness,
        },
    ))
}

/// What an agent did, newest first: structured kinds and names, never sentences (C-1083).
#[utoipa::path(get, path = "/agents/{agent}/timeline", operation_id = "getAgentTimeline", tag = "timeline",
    params(("agent" = Uuid, Path, description = "Agent id"), TimelineQuery),
    responses((status = 200, body = TimelinePage), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 503, description = "audit_unavailable", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn timeline(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Query(q): Query<TimelineQuery>,
) -> Result<Json<TimelinePage>, ApiError> {
    // The `Query` extractor has already checked the query's shape; authorise before checking its
    // values (limit, cursor, kind, outcome), so a caller without access learns nothing from them
    // (FIX-181 F7).
    let (viewer, agent) = resolve(&s, &c, agent).await?;
    let limit = q.limit.unwrap_or(feed::DEFAULT_LIMIT);
    if !(1..=feed::MAX_LIMIT).contains(&limit) {
        return Err(ApiError::invalid_field(
            "Invalid limit.",
            "/limit",
            "must be between 1 and 100",
        ));
    }
    let cursor = q.cursor.as_deref().map(Cursor::decode).transpose()?;
    let filter = Filter::parse(q.kind.as_deref(), q.outcome.as_deref())?;
    let f = Feed::new(&s, viewer, agent).await?;
    let (items, next) = f.page(cursor.as_ref(), limit as usize, &filter).await?;
    Ok(Json(TimelinePage {
        items,
        next_cursor: next.map(|c| c.encode()),
    }))
}

/// Server-sent events: `row` (`{op: "upsert", item}`), `request` (with `technical=true`) and
/// `resync`; no SSE ids (C-1084).
#[utoipa::path(get, path = "/agents/{agent}/timeline/stream", operation_id = "streamAgentTimeline", tag = "timeline",
    params(("agent" = Uuid, Path, description = "Agent id"), StreamQuery),
    responses((status = 200, content_type = "text/event-stream", body = TimelineStreamEvents),
        (status = 401, body = ErrorEnvelope), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 429, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn timeline_stream(
    c: SessionCaller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Query(q): Query<StreamQuery>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let (viewer, agent) = resolve(&s, &c.0, agent).await?;
    let slot = s.timeline_bus.acquire(c.user_id)?;
    let keepalive = s.sse_keepalive;
    let gone = crate::http::auth::revoked(s.clone(), c.0.clone());
    let st = Stream::open(
        s.clone(),
        c.0.clone(),
        viewer,
        agent,
        q.technical == Some(true),
        slot,
    )
    .await?;
    let events = futures::StreamExt::take_until(st.into_events(), gone);
    Ok(Sse::new(events).keep_alive(KeepAlive::new().interval(keepalive)))
}
