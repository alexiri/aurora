use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::http::auth::{Caller, SessionCaller};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use aurora_core::ApiError;
use axum::extract::State;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_governance, put_governance))
        .routes(routes!(get_effective))
        .routes(routes!(get_instance))
}

/// Read the organization governance document (owner/admin).
#[utoipa::path(
    get, path = "/orgs/{org}/governance", operation_id = "getOrgGovernance", tag = "governance",
    params(("org" = Uuid, Path, description = "Organization id")),
    responses((status = 200, description = "Document and revision", body = GovernanceDocument),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn get_governance(
    caller: Caller,
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
) -> Result<Json<GovernanceDocument>, ApiError> {
    caller.org(&state, org).await?.require_admin()?;
    Ok(Json(service::get(&state, org).await?))
}

/// Replace the governance document (owner/admin); re-evaluates stored LLM key origins.
#[utoipa::path(
    put, path = "/orgs/{org}/governance", operation_id = "putOrgGovernance", tag = "governance",
    params(("org" = Uuid, Path, description = "Organization id")),
    request_body = PutGovernance,
    responses((status = 200, description = "Stored document", body = GovernanceDocument),
              (status = 400, description = "Invalid", body = ErrorEnvelope),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope),
              (status = 409, description = "stale_revision", body = ErrorEnvelope),
              (status = 422, description = "governance_invalid | governance_below_instance_minimum (details.fields)", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn put_governance(
    caller: Caller,
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
    Json(req): Json<PutGovernance>,
) -> Result<Json<GovernanceDocument>, ApiError> {
    caller.org(&state, org).await?.require_admin()?;
    let a = service::put(
        &state,
        org,
        req,
        crate::audit::AuditActor::from_caller(&caller),
    )
    .await?;
    Ok(Json(a.out))
}

/// Effective governance (instance and org) with sources (any org member).
#[utoipa::path(
    get, path = "/orgs/{org}/governance/effective", operation_id = "getOrgEffectiveGovernance", tag = "governance",
    params(("org" = Uuid, Path, description = "Organization id")),
    responses((status = 200, description = "Effective governance", body = EffectiveGovernance),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn get_effective(
    caller: Caller,
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
) -> Result<Json<EffectiveGovernance>, ApiError> {
    caller.org(&state, org).await?;
    Ok(Json(service::effective(&state, org).await?))
}

/// Instance governance ceiling.
#[utoipa::path(
    get, path = "/governance/instance", operation_id = "getInstanceGovernance", tag = "governance",
    responses((status = 200, description = "Instance ceiling", body = InstanceGovernance)),
    security(("session" = []))
)]
async fn get_instance(
    _caller: SessionCaller,
    State(state): State<AppState>,
) -> Json<InstanceGovernance> {
    Json(service::instance(&state))
}
