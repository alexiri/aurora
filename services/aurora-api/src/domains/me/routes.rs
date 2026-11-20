use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::audit::{self, AuditEvent};
use crate::http::auth::{Caller, Principal, SessionCaller};
use crate::http::client_ip::ClientIp;
use crate::http::error::ErrorEnvelope;
use crate::http::json::Json;
use aurora_core::ApiError;
use axum::Extension;
use axum::extract::State;
use axum::http::StatusCode;
use utoipa_axum::{router::OpenApiRouter, routes};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_me, patch_me, delete_me))
        .routes(routes!(my_chat_platforms))
}

/// (P5-REFINE-6, C-1089) The chat platforms the caller can link: the connected integrations of
/// their organizations, one per kind, with the bot to message. No workspace, channel or token data.
#[utoipa::path(get, path = "/me/chat-platforms", operation_id = "listMyChatPlatforms", tag = "me",
    responses((status = 200, body = MyChatPlatforms), (status = 401, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn my_chat_platforms(
    c: Caller,
    State(s): State<AppState>,
) -> Result<Json<MyChatPlatforms>, ApiError> {
    let only_org = match c.principal {
        Principal::ApiKey { org_id, .. } => Some(org_id),
        _ => None,
    };
    let rows =
        aurora_db::repo::channel_integrations::platforms_for_user(&s.pool, c.user_id, only_org)
            .await?;
    Ok(Json(MyChatPlatforms::by_kind(rows)))
}

/// The signed-in user with memberships and usable workspaces.
#[utoipa::path(get, path = "/me", operation_id = "getMe", tag = "me",
    responses((status = 200, body = Me), (status = 401, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_me(
    c: Caller,
    State(s): State<AppState>,
    Extension(ip): Extension<ClientIp>,
) -> Result<Json<Me>, ApiError> {
    let mut me = service::load_me(&s, c.user_id).await?;
    me.client_address = ip.0.map(|a| a.to_string());
    Ok(Json(me))
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
    Extension(ip): Extension<ClientIp>,
    Json(req): Json<PatchMeRequest>,
) -> Result<Json<Me>, ApiError> {
    let mut me = service::patch_me(&s, c.user_id, req).await?;
    me.client_address = ip.0.map(|a| a.to_string());
    audit::record(
        &s,
        AuditEvent::by(&c, "user.update").target("user", c.user_id),
    );
    Ok(Json(me))
}

/// Delete the caller's own account (session only; no re-authentication, FIX-157, D-317).
#[utoipa::path(delete, path = "/me", operation_id = "deleteMe", tag = "me",
    responses((status = 204, description = "Deleted"), (status = 403, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn delete_me(c: SessionCaller, State(s): State<AppState>) -> Result<StatusCode, ApiError> {
    let Principal::Session { .. } = c.principal else {
        return Err(ApiError::forbidden("Session required."));
    };
    let ev = AuditEvent::by(&c, "user.delete").target("user", c.user_id);
    service::delete_me(&s, c.user_id, &ev).await?;
    Ok(StatusCode::NO_CONTENT)
}
