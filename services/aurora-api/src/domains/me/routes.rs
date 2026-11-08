use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::http::auth::{Caller, Principal, SessionCaller};
use crate::http::error::ErrorEnvelope;
use crate::http::json::Json;
use aurora_core::ApiError;
use axum::extract::State;
use axum::http::StatusCode;
use utoipa_axum::{router::OpenApiRouter, routes};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_me, patch_me, delete_me))
}

/// The signed-in user with memberships and usable workspaces.
#[utoipa::path(get, path = "/me", operation_id = "getMe", tag = "me",
    responses((status = 200, body = Me), (status = 401, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_me(c: Caller, State(s): State<AppState>) -> Result<Json<Me>, ApiError> {
    Ok(Json(service::load_me(&s, c.user_id).await?))
}

/// Update display name and/or default organization.
#[utoipa::path(patch, path = "/me", operation_id = "patchMe", tag = "me",
    request_body = PatchMeRequest,
    responses((status = 200, body = Me), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn patch_me(
    c: Caller,
    State(s): State<AppState>,
    Json(req): Json<PatchMeRequest>,
) -> Result<Json<Me>, ApiError> {
    let me = service::patch_me(&s, c.user_id, req).await?;
    audit::record(
        &s,
        AuditEvent::new(AuditActor::from_caller(&c), "user.update", Outcome::Success)
            .target("user", c.user_id),
    );
    Ok(Json(me))
}

/// Delete the caller's own account (needs a recent OIDC re-authentication, else 412 `reauth_required`).
#[utoipa::path(delete, path = "/me", operation_id = "deleteMe", tag = "me",
    responses((status = 204, description = "Deleted"), (status = 403, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 412, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn delete_me(c: SessionCaller, State(s): State<AppState>) -> Result<StatusCode, ApiError> {
    let Principal::Session { id } = c.principal else {
        return Err(ApiError::forbidden("Session required."));
    };
    let ev = AuditEvent::new(AuditActor::from_caller(&c), "user.delete", Outcome::Success)
        .target("user", c.user_id);
    service::delete_me(&s, c.user_id, id, &ev).await?;
    Ok(StatusCode::NO_CONTENT)
}
