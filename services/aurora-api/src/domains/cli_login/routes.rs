use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::http::auth::SessionCaller;
use crate::http::client_ip::ClientIp;
use crate::http::error::ErrorEnvelope;
use crate::http::json::Json;
use crate::http::ratelimit::Quota;
use aurora_core::ApiError;
use axum::Extension;
use axum::extract::State;
use axum::http::StatusCode;
use std::time::Duration;
use utoipa_axum::{router::OpenApiRouter, routes};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(start))
        .routes(routes!(token))
        .routes(routes!(approve))
}

/// Begin a CLI device login.
#[utoipa::path(post, path = "/auth/cli", operation_id = "startCliLogin", tag = "auth",
    request_body = StartRequest,
    responses((status = 200, body = StartResponse), (status = 429, body = ErrorEnvelope)))]
async fn start(
    State(s): State<AppState>,
    Extension(ip): Extension<ClientIp>,
    Json(req): Json<StartRequest>,
) -> Result<Json<StartResponse>, ApiError> {
    let key = ip.0.map_or_else(|| "unknown".to_owned(), |i| i.to_string());
    s.rate_limiter.check_quota(
        "cli_start",
        &key,
        Quota {
            capacity: 10,
            per: Duration::from_secs(60),
        },
    )?;
    Ok(Json(service::start(&s, req).await?))
}

/// Poll for the API key; the key is returned exactly once.
#[utoipa::path(post, path = "/auth/cli/token", operation_id = "pollCliLogin", tag = "auth",
    request_body = TokenRequest,
    responses((status = 200, body = TokenResponse), (status = 400, body = ErrorEnvelope)))]
async fn token(
    State(s): State<AppState>,
    Json(req): Json<TokenRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    let c = service::token(&s, &req.device_code).await?;
    Ok(Json(c.response))
}

/// Approve a CLI login for a workspace (session only).
#[utoipa::path(post, path = "/auth/cli/approve", operation_id = "approveCliLogin", tag = "auth",
    request_body = ApproveRequest,
    responses((status = 204, description = "Approved"), (status = 400, body = ErrorEnvelope),
        (status = 403, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn approve(
    c: SessionCaller,
    State(s): State<AppState>,
    Json(req): Json<ApproveRequest>,
) -> Result<StatusCode, ApiError> {
    service::approve(&s, &c, req).await?;
    Ok(StatusCode::NO_CONTENT)
}
