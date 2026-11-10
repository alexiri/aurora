use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::http::auth::{Caller, InstanceAdmin};
use crate::http::error::ErrorEnvelope;
use crate::http::json::Json;
use aurora_core::ApiError;
use axum::extract::State;
use utoipa_axum::{router::OpenApiRouter, routes};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_onboarding, patch_onboarding))
        .routes(routes!(setup))
}

/// The caller's onboarding progress (Welcome seen, dismissed guides, tour).
#[utoipa::path(get, path = "/me/onboarding", operation_id = "getMeOnboarding", tag = "me",
    responses((status = 200, body = MeOnboarding), (status = 401, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_onboarding(
    c: Caller,
    State(s): State<AppState>,
) -> Result<Json<MeOnboarding>, ApiError> {
    Ok(Json(service::get_me(&s, c.user_id).await?))
}

/// Change the caller's onboarding progress; only the fields sent change.
#[utoipa::path(patch, path = "/me/onboarding", operation_id = "patchMeOnboarding", tag = "me",
    request_body = PatchOnboarding,
    responses((status = 200, body = MeOnboarding), (status = 400, body = ErrorEnvelope),
        (status = 401, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn patch_onboarding(
    c: Caller,
    State(s): State<AppState>,
    Json(req): Json<PatchOnboarding>,
) -> Result<Json<MeOnboarding>, ApiError> {
    Ok(Json(service::patch_me(&s, c.user_id, req).await?))
}

/// Status of each instance setup step with reason codes (instance administrators).
#[utoipa::path(get, path = "/admin/setup", operation_id = "adminSetup", tag = "admin",
    responses((status = 200, body = SetupStatus), (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn setup(c: InstanceAdmin, State(s): State<AppState>) -> Result<Json<SetupStatus>, ApiError> {
    Ok(Json(service::setup(&s, c.user_id).await?))
}
