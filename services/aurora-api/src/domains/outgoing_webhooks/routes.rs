use super::dto::{
    PreviewRequest, PreviewResponse, PutWebhookRequest, PutWebhookResponse, TestRequest,
    TestResponse, WebhookView,
};
use super::service;
use crate::app::AppState;
use crate::http::auth::Caller;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use aurora_core::ApiError;
use aurora_webhook::WebhookPreset;
use axum::extract::State;
use serde_json::Value;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_webhook, put_webhook))
        .routes(routes!(test_webhook))
        .routes(routes!(preview_webhook))
        .routes(routes!(list_presets))
}

/// The configuration of an outgoing webhook (C-909): the template, slots, headers, limits, the
/// host and the **names** of its secret values. Never the URL, a secret value or the signing
/// secret. `can_own` of the connection's workspace (owner/admin for an organization's).
#[utoipa::path(
    get, path = "/connections/{id}/webhook", operation_id = "getConnectionWebhook", tag = "webhooks",
    params(("id" = Uuid, Path, description = "Connection id")),
    responses((status = 200, body = WebhookView), (status = 403, body = ErrorEnvelope),
              (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn get_webhook(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<WebhookView>, ApiError> {
    Ok(Json(service::get(&state, &caller, id).await?))
}

/// Replace the configuration (C-909). `secret_values`: a value sets, `null` deletes, an absent name
/// is kept. `signing_secret.mode`: `keep`, `generate`, `provided` or `off`. A change of secrets bumps
/// the credential version; any change reaches the gateway at once. Audited
/// `connection.webhook.update`.
#[utoipa::path(
    put, path = "/connections/{id}/webhook", operation_id = "putConnectionWebhook", tag = "webhooks",
    params(("id" = Uuid, Path, description = "Connection id")),
    request_body = PutWebhookRequest,
    responses((status = 200, body = PutWebhookResponse),
              (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
              (status = 409, body = ErrorEnvelope),
              (status = 422, description = "`webhook_config_invalid` with `fields[{path, code}]`", body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn put_webhook(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> Result<Json<PutWebhookResponse>, ApiError> {
    Ok(Json(service::put(&state, &caller, id, body).await?))
}

/// Send the webhook now, or with `dry_run` show the stored configuration's rendering (C-909). The
/// person is the actor; the same renderer, destination guard, redirect rule and rate counters as the
/// gateway. The answer is capped at 4 KiB and echo-checked. Audited `webhook.test_sent`.
#[utoipa::path(
    post, path = "/connections/{id}/webhook/test", operation_id = "testConnectionWebhook", tag = "webhooks",
    params(("id" = Uuid, Path, description = "Connection id")),
    request_body = TestRequest,
    responses((status = 200, body = TestResponse),
              (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
              (status = 422, body = ErrorEnvelope), (status = 429, body = ErrorEnvelope),
              (status = 502, body = ErrorEnvelope), (status = 504, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn test_webhook(
    caller: Caller,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<TestRequest>,
) -> Result<Json<TestResponse>, ApiError> {
    Ok(Json(service::test(&state, &caller, id, req).await?))
}

/// Render a configuration for sample values with no network; secret values appear as
/// `«secret:name»` (C-909). Needs a session.
#[utoipa::path(
    post, path = "/apps/webhook/webhook-preview", operation_id = "previewWebhook", tag = "webhooks",
    request_body = PreviewRequest,
    responses((status = 200, body = PreviewResponse), (status = 422, body = ErrorEnvelope)),
    security(("session" = []))
)]
async fn preview_webhook(
    _caller: Caller,
    Json(body): Json<Value>,
) -> Result<Json<PreviewResponse>, ApiError> {
    Ok(Json(service::preview(body)?))
}

/// The presets of the installed package version of a send-mode app (C-909).
#[utoipa::path(
    get, path = "/apps/{app}/webhook-presets", operation_id = "listWebhookPresets", tag = "webhooks",
    params(("app" = String, Path, description = "App id")),
    responses((status = 200, body = Vec<WebhookPreset>), (status = 404, body = ErrorEnvelope)),
    security(("session" = []), ("api_key" = []))
)]
async fn list_presets(
    _caller: Caller,
    State(state): State<AppState>,
    Path(app): Path<String>,
) -> Result<Json<Vec<WebhookPreset>>, ApiError> {
    Ok(Json(service::presets(&state, &app).await?))
}
