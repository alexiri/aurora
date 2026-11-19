use super::dto::{AppSetupRequest, AppSetupRequestList, ListQuery};
use super::service;
use crate::app::AppState;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use aurora_core::ApiError;
use axum::extract::State;
use axum::http::StatusCode;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(ask_setup, withdraw_setup))
        .routes(routes!(list_requests))
        .routes(routes!(decline_request))
}

/// Ask the organization's owners and admins to make an app ready (C-1030). `can_own` of the workspace.
/// Idempotent per person; 409 `app_ready` when the app is ready already; 429 after 20 asks in an hour.
#[utoipa::path(post, path = "/workspaces/{ws}/apps/{app}/setup-request", operation_id = "askAppSetup", tag = "apps",
    params(("ws" = Uuid, Path, description = "Workspace id"), ("app" = String, Path, description = "App id")),
    responses((status = 200, body = AppSetupRequest), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope),
        (status = 409, description = "app_ready", body = ErrorEnvelope),
        (status = 429, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn ask_setup(
    c: Caller,
    State(s): State<AppState>,
    Path((ws, app)): Path<(Uuid, String)>,
) -> Result<Json<AppSetupRequest>, ApiError> {
    Ok(Json(service::ask(&s, &c, ws, &app).await?))
}

/// Withdraw the caller's ask; the request closes when no asker is left.
#[utoipa::path(delete, path = "/workspaces/{ws}/apps/{app}/setup-request", operation_id = "withdrawAppSetup", tag = "apps",
    params(("ws" = Uuid, Path, description = "Workspace id"), ("app" = String, Path, description = "App id")),
    responses((status = 204, description = "Withdrawn"), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn withdraw_setup(
    c: Caller,
    State(s): State<AppState>,
    Path((ws, app)): Path<(Uuid, String)>,
) -> Result<StatusCode, ApiError> {
    service::withdraw(&s, &c, ws, &app).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The organization's app setup requests (`status=open` by default), the latest ask first. Owners and
/// admins only; anyone else gets 404.
#[utoipa::path(get, path = "/orgs/{org}/app-setup-requests", operation_id = "listAppSetupRequests", tag = "apps",
    params(("org" = Uuid, Path, description = "Organization id"), ListQuery),
    responses((status = 200, body = AppSetupRequestList), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_requests(
    c: Caller,
    State(s): State<AppState>,
    Path(org): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> Result<Json<AppSetupRequestList>, ApiError> {
    Ok(Json(AppSetupRequestList {
        items: service::list(&s, &c, org, q.status.as_deref()).await?,
    }))
}

/// "Not now": closes the request as declined; each asker is told once. Owners and admins only.
#[utoipa::path(post, path = "/orgs/{org}/app-setup-requests/{id}/decline", operation_id = "declineAppSetupRequest", tag = "apps",
    params(("org" = Uuid, Path, description = "Organization id"), ("id" = Uuid, Path, description = "Request id")),
    responses((status = 200, body = AppSetupRequest), (status = 404, body = ErrorEnvelope),
        (status = 409, description = "already_closed", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn decline_request(
    c: Caller,
    State(s): State<AppState>,
    Path((org, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<AppSetupRequest>, ApiError> {
    Ok(Json(service::decline(&s, &c, org, id).await?))
}
