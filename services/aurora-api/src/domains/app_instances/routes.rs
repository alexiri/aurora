use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::audit::AuditActor;
use crate::http::auth::{Caller, InstanceAdmin};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path, Query};
use crate::http::pagination::Page;
use aurora_core::ApiError;
use aurora_db::repo::app_instances::{self as repo, InstanceRow};
use axum::extract::State;
use axum::http::StatusCode;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_org_instances, create_org_instance))
        .routes(routes!(list_instance_instances, create_instance_instance))
        .routes(routes!(patch_instance, delete_instance))
}

/// List the app instances usable in an organization: the instance-wide ones and its own (any
/// member).
#[utoipa::path(
    get, path = "/orgs/{org}/app-instances", operation_id = "listOrgAppInstances", tag = "app-instances",
    params(("org" = Uuid, Path, description = "Organization id"), ListQuery),
    responses((status = 200, description = "Instances", body = Page<AppInstance>),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn list_org_instances(
    caller: Caller,
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Page<AppInstance>>, ApiError> {
    caller.org(&state, org).await?;
    Ok(Json(service::list_for_org(&state, org, &q).await?))
}

/// Register a self-hosted instance of an app for an organization (owner/admin). The host is the
/// only place the app's credentials may be sent for connections bound to it (SEC-59).
#[utoipa::path(
    post, path = "/orgs/{org}/app-instances", operation_id = "createOrgAppInstance", tag = "app-instances",
    params(("org" = Uuid, Path, description = "Organization id")),
    request_body = CreateAppInstanceRequest,
    responses((status = 201, description = "Registered", body = AppInstance),
              (status = 400, description = "Invalid", body = ErrorEnvelope),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope),
              (status = 422, description = "app_not_instance_bound, instance_host_invalid, instance_host_conflict or instance_probe_failed", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn create_org_instance(
    caller: Caller,
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
    Json(req): Json<CreateAppInstanceRequest>,
) -> Result<(StatusCode, Json<AppInstance>), ApiError> {
    caller.org(&state, org).await?.require_admin()?;
    let row = service::create(&state, &AuditActor::from_caller(&caller), Some(org), req).await?;
    Ok((StatusCode::CREATED, Json(row.into())))
}

/// List instance-wide app instances (instance admin).
#[utoipa::path(
    get, path = "/admin/app-instances", operation_id = "listInstanceAppInstances", tag = "app-instances",
    params(ListQuery),
    responses((status = 200, description = "Instances", body = Page<AppInstance>),
              (status = 403, description = "Forbidden", body = ErrorEnvelope)),
    security(("session" = []))
)]
async fn list_instance_instances(
    _admin: InstanceAdmin,
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Page<AppInstance>>, ApiError> {
    Ok(Json(service::list_instance_wide(&state, &q).await?))
}

/// Register an instance-wide app instance (instance admin).
#[utoipa::path(
    post, path = "/admin/app-instances", operation_id = "createInstanceAppInstance", tag = "app-instances",
    request_body = CreateAppInstanceRequest,
    responses((status = 201, description = "Registered", body = AppInstance),
              (status = 400, description = "Invalid", body = ErrorEnvelope),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 422, description = "app_not_instance_bound, instance_host_invalid, instance_host_conflict or instance_probe_failed", body = ErrorEnvelope)),
    security(("session" = []))
)]
async fn create_instance_instance(
    InstanceAdmin(caller): InstanceAdmin,
    State(state): State<AppState>,
    Json(req): Json<CreateAppInstanceRequest>,
) -> Result<(StatusCode, Json<AppInstance>), ApiError> {
    let row = service::create(&state, &AuditActor::instance_admin(&caller), None, req).await?;
    Ok((StatusCode::CREATED, Json(row.into())))
}

/// Authority over an instance: the owners/admins of its org, or an instance admin for an
/// instance-wide one (403 otherwise; 404 when invisible).
async fn writable(
    caller: &Caller,
    state: &AppState,
    id: Uuid,
) -> Result<(InstanceRow, AuditActor), ApiError> {
    let row = repo::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("App instance not found."))?;
    let actor = match row.org_id {
        Some(org) => {
            caller.org(state, org).await?.require_admin()?;
            AuditActor::from_caller(caller)
        }
        None => {
            if !(caller.is_instance_admin && caller.is_session()) {
                return Err(ApiError::forbidden("Instance administrator required."));
            }
            AuditActor::instance_admin(caller)
        }
    };
    Ok((row, actor))
}

/// Rename an instance or enable/disable injection on it (the host is immutable).
#[utoipa::path(
    patch, path = "/app-instances/{id}", operation_id = "updateAppInstance", tag = "app-instances",
    params(("id" = Uuid, Path, description = "App instance id")),
    request_body = PatchAppInstanceRequest,
    responses((status = 200, description = "Updated", body = AppInstance),
              (status = 400, description = "Invalid", body = ErrorEnvelope),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn patch_instance(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<PatchAppInstanceRequest>,
) -> Result<Json<AppInstance>, ApiError> {
    let (row, actor) = writable(&caller, &state, id).await?;
    Ok(Json(
        service::patch(&state, &actor, &row, req).await?.into(),
    ))
}

/// Delete an instance: its connections are disabled (`instance_deleted`) and its OAuth clients
/// removed in the same transaction; nothing is injected on its host again.
#[utoipa::path(
    delete, path = "/app-instances/{id}", operation_id = "deleteAppInstance", tag = "app-instances",
    params(("id" = Uuid, Path, description = "App instance id")),
    responses((status = 204, description = "Deleted"),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn delete_instance(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (row, actor) = writable(&caller, &state, id).await?;
    service::delete(&state, &actor, &row).await?;
    Ok(StatusCode::NO_CONTENT)
}
