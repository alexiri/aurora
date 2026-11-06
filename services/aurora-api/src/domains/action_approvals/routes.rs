use super::dto::{ActionApproval, ActionDecisionBody, ListQuery};
use super::service;
use crate::app::AppState;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::{Page, Pagination};
use aurora_core::ApiError;
use aurora_db::repo::action_approvals as repo;
use axum::extract::State;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_action_approvals))
        .routes(routes!(decide_action_approval))
}

/// Action approvals of a workspace, newest first.
#[utoipa::path(get, path = "/workspaces/{ws}/action-approvals", operation_id = "listActionApprovals", tag = "action-approvals",
    params(("ws" = Uuid, Path, description = "Workspace id"), ListQuery),
    responses((status = 200, body = Page<ActionApproval>), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_action_approvals(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Page<ActionApproval>>, ApiError> {
    c.workspace(&s, ws).await?;
    service::validate_status(q.status.as_deref())?;
    let p = Pagination::new(q.limit, q.cursor.as_deref())?;
    let rows = repo::list_workspace(
        &s.pool,
        ws,
        q.status.as_deref(),
        q.agent_id,
        p.after.map(|a| (a.t, a.id)),
        p.fetch(),
    )
    .await?;
    let page = Page::from_rows(rows, p.limit, |r| (r.created_at, r.id));
    Ok(Json(Page {
        items: page.items.into_iter().map(ActionApproval::from).collect(),
        next_cursor: page.next_cursor,
    }))
}

/// Approve or deny an action approval; the first decision wins.
#[utoipa::path(post, path = "/action-approvals/{id}/decision", operation_id = "decideActionApproval", tag = "action-approvals",
    params(("id" = Uuid, Path, description = "Action approval id")),
    request_body = ActionDecisionBody,
    responses((status = 200, body = ActionApproval), (status = 404, body = ErrorEnvelope),
        (status = 409, description = "already_decided", body = ErrorEnvelope),
        (status = 500, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn decide_action_approval(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(b): Json<ActionDecisionBody>,
) -> Result<Json<ActionApproval>, ApiError> {
    Ok(Json(service::decide(&s, &c, id, b.decision).await?))
}
