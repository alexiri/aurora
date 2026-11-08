use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use aurora_core::ApiError;
use axum::extract::State;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_availability, put_availability))
}

/// Posture and allow-list (owner/admin; I-27).
#[utoipa::path(
    get, path = "/orgs/{org}/app-availability", operation_id = "getOrgAppAvailability", tag = "availability",
    params(("org" = Uuid, Path, description = "Organization id")),
    responses((status = 200, description = "Availability", body = AppAvailability),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn get_availability(
    caller: Caller,
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
) -> Result<Json<AppAvailability>, ApiError> {
    caller.org(&state, org).await?.require_admin()?;
    Ok(Json(service::get(&state, org).await?))
}

/// Replace posture and allow-list (owner/admin).
#[utoipa::path(
    put, path = "/orgs/{org}/app-availability", operation_id = "putOrgAppAvailability", tag = "availability",
    params(("org" = Uuid, Path, description = "Organization id")),
    request_body = AppAvailability,
    responses((status = 200, description = "Availability", body = AppAvailability),
              (status = 400, description = "Invalid", body = ErrorEnvelope),
              (status = 403, description = "Forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn put_availability(
    caller: Caller,
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
    Json(req): Json<AppAvailability>,
) -> Result<Json<AppAvailability>, ApiError> {
    caller.org(&state, org).await?.require_admin()?;
    let out = service::put(
        &state,
        org,
        req,
        crate::audit::AuditActor::from_caller(&caller),
    )
    .await?;
    Ok(Json(out))
}
