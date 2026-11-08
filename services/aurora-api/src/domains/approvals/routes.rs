use super::dto::{
    Approval, ApprovalEvent, BatchDecided, BatchDecisionBody, DecisionBody, ListQuery, PendingList,
    PendingQuery,
};
use super::{bus::BusMsg, service};
use crate::app::AppState;
use crate::http::auth::{Caller, SessionCaller, require_approve};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, Pagination};
use aurora_core::ApiError;
use aurora_db::repo::approvals as repo;
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::Stream;
use std::convert::Infallible;
use tokio::sync::broadcast::error::RecvError;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_approvals))
        .routes(routes!(pending))
        .routes(routes!(stream))
        .routes(routes!(decide))
        .routes(routes!(decide_batch))
}

/// Approvals of a workspace, newest first.
#[utoipa::path(get, path = "/workspaces/{ws}/approvals", operation_id = "listApprovals", tag = "approvals",
    params(("ws" = Uuid, Path, description = "Workspace id"), ListQuery),
    responses((status = 200, body = Page<Approval>), (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_approvals(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Page<Approval>>, ApiError> {
    require_approve(&c.workspace(&s, ws).await?)?;
    service::validate_status(q.status.as_deref())?;
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    let rows = repo::list_workspace(
        &s.pool,
        c.user_id,
        ws,
        q.status.as_deref(),
        p.after.map(|a| (a.t, a.id)),
        p.fetch(),
    )
    .await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    Ok(Json(Page {
        items: page.items.into_iter().map(Approval::from).collect(),
        next_cursor: page.next_cursor,
    }))
}

/// Pending approvals the caller may decide; `wait` (<=30 s) long-polls for a new one.
#[utoipa::path(get, path = "/approvals/pending", operation_id = "listPendingApprovals", tag = "approvals",
    params(PendingQuery),
    responses((status = 200, body = PendingList), (status = 401, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn pending(
    c: Caller,
    State(s): State<AppState>,
    Query(q): Query<PendingQuery>,
) -> Result<Json<PendingList>, ApiError> {
    let items = service::pending(&s, &c, q.wait.unwrap_or(0)).await?;
    Ok(Json(PendingList { items }))
}

/// Server-sent events `approval` (`created` | `decided` | `expired`), no SSE ids (C-10).
#[utoipa::path(get, path = "/approvals/stream", operation_id = "streamApprovals", tag = "approvals",
    responses((status = 200, content_type = "text/event-stream", body = ApprovalEvent),
        (status = 401, body = ErrorEnvelope), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn stream(
    c: SessionCaller,
    State(s): State<AppState>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let rx = s.approval_bus.subscribe(&s.pool).await?;
    let user = c.user_id;
    let keepalive = s.sse_keepalive;
    let gone = crate::http::auth::revoked(s.clone(), c.0.clone());
    // (RFE-011a) An open stream is a live delivery surface for its user until it closes.
    let watch = super::notify::watch(&s, user, None, None).await;
    let events = futures::stream::unfold((rx, s, watch), move |(mut rx, s, watch)| async move {
        loop {
            match rx.recv().await {
                Ok(BusMsg::Event {
                    id,
                    workspace_id,
                    event,
                }) => {
                    let Some(approval) = service::stream_item(&s, user, id, workspace_id).await
                    else {
                        continue;
                    };
                    let notice = if event == "created" {
                        service::notice_for(&s, &approval).await
                    } else {
                        None
                    };
                    let data = ApprovalEvent {
                        event,
                        approval,
                        notice,
                    };
                    let ev = Event::default()
                        .event("approval")
                        .json_data(&data)
                        .unwrap_or_else(|_| Event::default().comment("encode error"));
                    return Some((Ok(ev), (rx, s, watch)));
                }
                Ok(BusMsg::Resync) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return None,
            }
        }
    });
    let events = futures::StreamExt::take_until(events, gone);
    Ok(Sse::new(events).keep_alive(KeepAlive::new().interval(keepalive)))
}

/// Decide one approval; first decision wins.
#[utoipa::path(post, path = "/approvals/{id}/decision", operation_id = "decideApproval", tag = "approvals",
    params(("id" = Uuid, Path, description = "Approval id")),
    request_body = DecisionBody,
    responses((status = 200, body = Approval), (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn decide(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(b): Json<DecisionBody>,
) -> Result<Json<Approval>, ApiError> {
    Ok(Json(service::decide(&s, &c, id, b.decision).await?))
}

/// Decide all pending approvals of a batch for one agent that the caller may decide.
#[utoipa::path(post, path = "/approvals/batches/{batch_id}/decision", operation_id = "decideApprovalBatch", tag = "approvals",
    params(("batch_id" = String, Path, description = "Batch id")),
    request_body = BatchDecisionBody,
    responses((status = 200, body = BatchDecided), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn decide_batch(
    c: Caller,
    State(s): State<AppState>,
    Path(batch_id): Path<String>,
    Json(b): Json<BatchDecisionBody>,
) -> Result<Json<BatchDecided>, ApiError> {
    let decided = service::decide_batch(&s, &c, &batch_id, b.agent_id, b.decision).await?;
    Ok(Json(BatchDecided { decided }))
}
