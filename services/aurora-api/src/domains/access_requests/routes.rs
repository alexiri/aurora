use super::dto::{AccessRequest, ListQuery};
use super::service;
use crate::app::AppState;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use aurora_core::ApiError;
use axum::extract::State;
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_access_requests))
        .routes(routes!(dismiss_access_request))
}

#[derive(Serialize, ToSchema)]
pub struct AccessRequestList {
    pub items: Vec<AccessRequest>,
}

/// Agents' requests for access in a workspace (`status=open` by default), newest first. They feed
/// the banner above the Accounts list (C-984).
#[utoipa::path(get, path = "/workspaces/{ws}/access-requests", operation_id = "listAccessRequests", tag = "access-requests",
    params(("ws" = Uuid, Path, description = "Workspace id"), ListQuery),
    responses((status = 200, body = AccessRequestList), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_access_requests(
    c: Caller,
    State(s): State<AppState>,
    Path(ws): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> Result<Json<AccessRequestList>, ApiError> {
    c.workspace(&s, ws).await?;
    Ok(Json(AccessRequestList {
        items: service::list(&s, ws, q.status.as_deref()).await?,
    }))
}

/// "Not now": dismisses an open request. `can_own` of the workspace.
#[utoipa::path(post, path = "/access-requests/{id}/dismiss", operation_id = "dismissAccessRequest", tag = "access-requests",
    params(("id" = Uuid, Path, description = "Request id")),
    responses((status = 200, body = AccessRequest), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope),
        (status = 409, description = "already_resolved", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn dismiss_access_request(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<AccessRequest>, ApiError> {
    Ok(Json(service::dismiss(&s, &c, id).await?))
}
