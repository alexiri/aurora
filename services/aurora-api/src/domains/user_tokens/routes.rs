//! `/v1/me/user-tokens` (P4-UTOK, C-853, C-855). Reads take a session or the person's own key;
//! every write is session-only (SO). There is no route that creates an `allow` mode: a send is
//! always a held approval (SEC-113).
use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::http::auth::{Caller, SessionCaller};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use aurora_core::ApiError;
use axum::extract::State;
use axum::http::StatusCode;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_tokens, create_token))
        .routes(routes!(patch_token, delete_token))
        .routes(routes!(put_grant, delete_grant))
        .routes(routes!(list_sends))
}

/// The posts agents proposed to make as you, with their full text (yours only). Decide one with
/// `POST /action-approvals/{id}/decision`; no one else can.
#[utoipa::path(get, path = "/me/user-token-approvals", operation_id = "listOwnSendApprovals", tag = "user-tokens",
    params(OwnSendQuery),
    responses((status = 200, body = OwnSendApprovalList), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn list_sends(
    c: SessionCaller,
    State(s): State<AppState>,
    crate::http::json::Query(q): crate::http::json::Query<OwnSendQuery>,
) -> Result<Json<OwnSendApprovalList>, ApiError> {
    Ok(Json(service::own_sends(&s, &c, q.status.as_deref()).await?))
}

/// The caller's own tokens (never the value).
#[utoipa::path(get, path = "/me/user-tokens", operation_id = "listUserTokens", tag = "user-tokens",
    responses((status = 200, body = UserTokenList), (status = 401, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn list_tokens(
    c: Caller,
    State(s): State<AppState>,
) -> Result<Json<UserTokenList>, ApiError> {
    Ok(Json(service::list(&s, &c).await?))
}

/// Add a chat-platform token of your own (session only). It is sealed, verified against the
/// platform and compared with your own link before it is stored; the value is never returned.
#[utoipa::path(post, path = "/me/user-tokens", operation_id = "createUserToken", tag = "user-tokens",
    request_body = CreateUserTokenRequest,
    responses((status = 201, body = UserToken), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 409, body = ErrorEnvelope),
        (status = 422, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn create_token(
    c: SessionCaller,
    State(s): State<AppState>,
    Json(req): Json<CreateUserTokenRequest>,
) -> Result<(StatusCode, Json<UserToken>), ApiError> {
    Ok((
        StatusCode::CREATED,
        Json(service::create(&s, &c, req).await?),
    ))
}

/// Rename one of your tokens.
#[utoipa::path(patch, path = "/me/user-tokens/{id}", operation_id = "patchUserToken", tag = "user-tokens",
    params(("id" = Uuid, Path, description = "Token id")), request_body = PatchUserTokenRequest,
    responses((status = 200, body = UserToken), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn patch_token(
    c: SessionCaller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<PatchUserTokenRequest>,
) -> Result<Json<UserToken>, ApiError> {
    Ok(Json(service::rename(&s, &c, id, req).await?))
}

/// Revoke one of your tokens: agents stop posting as you and pending posts are cancelled.
#[utoipa::path(delete, path = "/me/user-tokens/{id}", operation_id = "deleteUserToken", tag = "user-tokens",
    params(("id" = Uuid, Path, description = "Token id")),
    responses((status = 204), (status = 404, body = ErrorEnvelope), (status = 503, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn delete_token(
    c: SessionCaller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    service::revoke(&s, &c, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Grant one agent the use of a token (idempotent).
#[utoipa::path(put, path = "/me/user-tokens/{id}/agents/{agent}", operation_id = "grantUserToken", tag = "user-tokens",
    params(("id" = Uuid, Path, description = "Token id"), ("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 204), (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn put_grant(
    c: SessionCaller,
    State(s): State<AppState>,
    Path((id, agent)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    service::grant(&s, &c, id, agent).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Remove an agent's grant (idempotent); its pending posts are cancelled.
#[utoipa::path(delete, path = "/me/user-tokens/{id}/agents/{agent}", operation_id = "ungrantUserToken", tag = "user-tokens",
    params(("id" = Uuid, Path, description = "Token id"), ("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 204), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn delete_grant(
    c: SessionCaller,
    State(s): State<AppState>,
    Path((id, agent)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    service::ungrant(&s, &c, id, agent).await?;
    Ok(StatusCode::NO_CONTENT)
}
