use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use aurora_core::ApiError;
use axum::extract::State;
use serde_json::json;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_availability, put_availability))
}

/// Posture and allow-list (org member).
#[utoipa::path(
    get, path = "/orgs/{org}/app-availability", operation_id = "getOrgAppAvailability", tag = "availability",
    params(("org" = Uuid, Path, description = "Organization id")),
    responses((status = 200, description = "Availability", body = AppAvailability),
              (status = 404, description = "Not found", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn get_availability(
    caller: Caller,
    State(state): State<AppState>,
    Path(org): Path<Uuid>,
) -> Result<Json<AppAvailability>, ApiError> {
    caller.org(&state, org).await?;
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
    let out = service::put(&state, org, req).await?;
    audit::record(
        &state,
        AuditEvent::new(
            AuditActor::from_caller(&caller),
            "org.app_availability.update",
            Outcome::Success,
        )
        .org(org)
        .target("organization", org)
        .metadata(json!({"posture": out.posture, "entries": out.allow.len()})),
    );
    Ok(Json(out))
}
