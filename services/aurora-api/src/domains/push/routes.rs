//! The Web Push routes (C-830, C-831). Each one answers 404 `push_unavailable` first, before
//! authentication, while `push.enabled` is off; the subscription routes are session only.
use super::dto::*;
use super::{push_unavailable, service};
use crate::app::AppState;
use crate::http::auth::{Caller, SessionCaller};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use aurora_core::ApiError;
use axum::extract::{FromRequestParts, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_config))
        .routes(routes!(list_subscriptions, create_subscription))
        .routes(routes!(update_subscription, delete_subscription))
        .routes(routes!(test_subscription))
}

/// Send one test notification to one of the caller's devices (session only). The outcome shows in
/// the device row (`last_success_at` / `last_failure_at`).
#[utoipa::path(post, path = "/me/push-subscriptions/{id}/test", operation_id = "testPushSubscription",
    tag = "push", params(("id" = Uuid, Path, description = "Subscription id")),
    responses((status = 202), (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 429, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn test_subscription(
    _on: PushOn,
    c: SessionCaller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    service::test(&s, &c, id).await?;
    Ok(StatusCode::ACCEPTED)
}

/// Placed first in every handler: 404 `push_unavailable` while push is off.
struct PushOn;

impl FromRequestParts<AppState> for PushOn {
    type Rejection = ApiError;

    async fn from_request_parts(_: &mut Parts, s: &AppState) -> Result<Self, ApiError> {
        if s.push.enabled {
            Ok(Self)
        } else {
            Err(push_unavailable())
        }
    }
}

/// The application-server key and the allowed push hosts.
#[utoipa::path(get, path = "/push/config", operation_id = "getPushConfig", tag = "push",
    responses((status = 200, body = PushConfig), (status = 401, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = [])))]
async fn get_config(
    _on: PushOn,
    _c: Caller,
    State(s): State<AppState>,
) -> Result<Json<PushConfig>, ApiError> {
    Ok(Json(service::config(&s).await?))
}

/// The caller's registered devices (session only).
#[utoipa::path(get, path = "/me/push-subscriptions", operation_id = "listPushSubscriptions",
    tag = "push",
    responses((status = 200, body = PushSubscriptionList), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn list_subscriptions(
    _on: PushOn,
    c: SessionCaller,
    State(s): State<AppState>,
) -> Result<Json<PushSubscriptionList>, ApiError> {
    Ok(Json(service::list(&s, &c).await?))
}

/// Register this browser (session only). 201 for a new registration, 200 when the endpoint
/// already belonged to the caller (the session is re-bound).
#[utoipa::path(post, path = "/me/push-subscriptions", operation_id = "createPushSubscription",
    tag = "push", request_body = CreatePushSubscription,
    responses((status = 201, body = PushSubscription), (status = 200, body = PushSubscription),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 409, body = ErrorEnvelope), (status = 422, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn create_subscription(
    _on: PushOn,
    c: SessionCaller,
    State(s): State<AppState>,
    Json(req): Json<CreatePushSubscription>,
) -> Result<(StatusCode, Json<PushSubscription>), ApiError> {
    let (sub, created) = service::create(&s, &c, req).await?;
    Ok((
        if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        Json(sub),
    ))
}

/// Rename, pause or change the details of one of the caller's devices (session only).
#[utoipa::path(patch, path = "/me/push-subscriptions/{id}", operation_id = "updatePushSubscription",
    tag = "push", params(("id" = Uuid, Path, description = "Subscription id")),
    request_body = UpdatePushSubscription,
    responses((status = 200, body = PushSubscription), (status = 403, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn update_subscription(
    _on: PushOn,
    c: SessionCaller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdatePushSubscription>,
) -> Result<Json<PushSubscription>, ApiError> {
    Ok(Json(service::patch(&s, &c, id, req).await?))
}

/// Remove one of the caller's devices (session only; idempotent).
#[utoipa::path(delete, path = "/me/push-subscriptions/{id}", operation_id = "deletePushSubscription",
    tag = "push", params(("id" = Uuid, Path, description = "Subscription id")),
    responses((status = 204), (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn delete_subscription(
    _on: PushOn,
    c: SessionCaller,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    service::delete(&s, &c, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
