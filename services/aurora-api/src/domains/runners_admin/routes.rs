use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::audit::AuditActor;
use crate::http::auth::InstanceAdmin;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use aurora_core::ApiError;
use axum::extract::State;
use http::StatusCode;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_runners, create_runner))
        .routes(routes!(delete_runner))
        .routes(routes!(rotate_runner_token))
        .routes(routes!(runner_start_failures))
}

/// List runners with replicas and capacity (instance administrators).
#[utoipa::path(get, path = "/admin/runners", operation_id = "adminListRunners", tag = "admin",
    responses((status = 200, body = RunnerList), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn list_runners(
    _c: InstanceAdmin,
    State(s): State<AppState>,
) -> Result<Json<RunnerList>, ApiError> {
    Ok(Json(service::list(&s).await?))
}

/// Provision a runner; the `aur_` token is returned once.
#[utoipa::path(post, path = "/admin/runners", operation_id = "adminCreateRunner", tag = "admin",
    request_body = CreateRunnerRequest,
    responses((status = 201, body = RunnerWithToken), (status = 403, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn create_runner(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Json(req): Json<CreateRunnerRequest>,
) -> Result<(StatusCode, Json<RunnerWithToken>), ApiError> {
    let out = service::create(&s, &req.name, &AuditActor::instance_admin(&c)).await?;
    Ok((StatusCode::CREATED, Json(out)))
}

/// Delete a runner (409 while it has running sandboxes).
#[utoipa::path(delete, path = "/admin/runners/{id}", operation_id = "adminDeleteRunner", tag = "admin",
    params(("id" = Uuid, Path, description = "Runner id")),
    responses((status = 204), (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn delete_runner(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    service::delete(&s, id, &AuditActor::instance_admin(&c)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Rotate the runner token; the new `aur_` token is returned once.
#[utoipa::path(post, path = "/admin/runners/{id}/token", operation_id = "adminRotateRunnerToken", tag = "admin",
    params(("id" = Uuid, Path, description = "Runner id")),
    responses((status = 200, body = RunnerWithToken), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn rotate_runner_token(
    c: InstanceAdmin,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<RunnerWithToken>, ApiError> {
    let out = service::rotate(&s, id, &AuditActor::instance_admin(&c)).await?;
    Ok(Json(out))
}

/// Recent start failures of a runner (instance administrators; FIX-025, C-586).
#[utoipa::path(get, path = "/admin/runners/{id}/start-failures", operation_id = "adminRunnerStartFailures", tag = "admin",
    params(("id" = Uuid, Path, description = "Runner id")),
    responses((status = 200, body = RunnerStartFailures), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn runner_start_failures(
    _c: InstanceAdmin,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<RunnerStartFailures>, ApiError> {
    Ok(Json(service::start_failures(&s, id).await?))
}
