use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::http::auth::InstanceAdmin;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Query};
use aurora_core::ApiError;
use axum::extract::State;
use utoipa_axum::{router::OpenApiRouter, routes};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_placement, put_placement))
        .routes(routes!(preview))
        .routes(routes!(choices))
}

/// Workspaces and org-level credentials the placement form may offer for an organization.
#[utoipa::path(get, path = "/admin/placement/choices", operation_id = "adminPlacementChoices",
    tag = "admin", params(ChoicesQuery),
    responses((status = 200, body = PlacementChoices), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn choices(
    _c: InstanceAdmin,
    State(s): State<AppState>,
    Query(q): Query<ChoicesQuery>,
) -> Result<Json<PlacementChoices>, ApiError> {
    Ok(Json(service::choices(&s, q.org_id).await?))
}

/// The new-user placement and what it would do today (instance administrators).
#[utoipa::path(get, path = "/admin/placement", operation_id = "adminGetPlacement", tag = "admin",
    responses((status = 200, body = PlacementView), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn get_placement(
    _c: InstanceAdmin,
    State(s): State<AppState>,
) -> Result<Json<PlacementView>, ApiError> {
    Ok(Json(service::view(&s).await?))
}

/// Save the new-user placement (instance administrators; audited).
#[utoipa::path(put, path = "/admin/placement", operation_id = "adminPutPlacement", tag = "admin",
    request_body = PutPlacement,
    responses((status = 200, body = PlacementView), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn put_placement(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Json(req): Json<PutPlacement>,
) -> Result<Json<PlacementView>, ApiError> {
    Ok(Json(service::put(&s, &c, req).await?))
}

/// Render a workspace name template (instance administrators).
#[utoipa::path(get, path = "/admin/placement/preview", operation_id = "adminPlacementPreview",
    tag = "admin", params(PreviewQuery),
    responses((status = 200, body = PlacementPreview), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn preview(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Query(q): Query<PreviewQuery>,
) -> Result<Json<PlacementPreview>, ApiError> {
    Ok(Json(service::preview(&s, &c, q).await?))
}
