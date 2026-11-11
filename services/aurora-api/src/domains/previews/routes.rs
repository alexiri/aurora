//! Preview share routes (phase 4, P4-PRV-1, D-144, D-145; C-540..C-549). Management routes:
//! session or API key with `can_use` of the agent's workspace (docs/05 §Port previews). While
//! `previews.enabled` is false every route answers 404 `previews_disabled`.
use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use aurora_core::ApiError;
use axum::extract::State;
use axum::http::StatusCode;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_previews, create_preview))
        .routes(routes!(get_preview, patch_preview, delete_preview))
        .routes(routes!(start_preview))
}

/// An agent's preview shares (C-540).
#[utoipa::path(get, path = "/agents/{agent}/previews", operation_id = "listAgentPreviews", tag = "previews",
    params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = PreviewShareList), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_previews(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<Json<PreviewShareList>, ApiError> {
    Ok(Json(service::list(&s, &c, agent).await?))
}

/// Create a share (C-541): `{ port, start_command?, cwd? }` → 201 `PreviewShare`.
#[utoipa::path(post, path = "/agents/{agent}/previews", operation_id = "createAgentPreview", tag = "previews",
    params(("agent" = Uuid, Path, description = "Agent id")),
    request_body = CreatePreviewShareRequest,
    responses((status = 201, body = PreviewShare), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope),
        (status = 423, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn create_preview(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Json(req): Json<CreatePreviewShareRequest>,
) -> Result<(StatusCode, Json<PreviewShare>), ApiError> {
    let (status, share) = service::create(&s, &c, agent, req).await?;
    Ok((status, Json(share)))
}

/// One share (C-542); 404 for a share of a workspace the caller cannot use.
#[utoipa::path(get, path = "/previews/{id}", operation_id = "getPreviewShare", tag = "previews",
    params(("id" = Uuid, Path, description = "Preview share id")),
    responses((status = 200, body = PreviewShare), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_preview(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<PreviewShare>, ApiError> {
    service::ensure_enabled(&s)?;
    let (row, _a) = service::load_share(&s, &c, id).await?;
    let mut conn = s.pool.acquire().await.map_err(aurora_db::DbError::from)?;
    Ok(Json(service::to_dto(&s, &mut conn, row).await?))
}

/// Patch state / start command / cwd (C-542); the port and the label are immutable.
#[utoipa::path(patch, path = "/previews/{id}", operation_id = "patchPreviewShare", tag = "previews",
    params(("id" = Uuid, Path, description = "Preview share id")),
    request_body = PatchPreviewShareRequest,
    responses((status = 200, body = PreviewShare), (status = 404, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope), (status = 423, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn patch_preview(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<PatchPreviewShareRequest>,
) -> Result<Json<PreviewShare>, ApiError> {
    Ok(Json(service::patch(&s, &c, id, req).await?))
}

/// Delete a share (C-542); the hostname is never reused.
#[utoipa::path(delete, path = "/previews/{id}", operation_id = "deletePreviewShare", tag = "previews",
    params(("id" = Uuid, Path, description = "Preview share id")),
    responses((status = 204, description = "Deleted"), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn delete_preview(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    service::delete(&s, &c, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Start the share's app now (C-544): wakes the sandbox if parked, enqueues the on-demand start.
#[utoipa::path(post, path = "/previews/{id}/start", operation_id = "startPreviewShare", tag = "previews",
    params(("id" = Uuid, Path, description = "Preview share id")),
    responses((status = 202, body = PreviewStartAccepted), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 423, body = ErrorEnvelope),
        (status = 429, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn start_preview(
    c: Caller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<PreviewStartAccepted>), ApiError> {
    let (status, body) = service::start(&s, &c, id).await?;
    Ok((status, Json(body)))
}
