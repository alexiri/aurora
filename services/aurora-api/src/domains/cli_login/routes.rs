use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::http::auth::SessionCaller;
use crate::http::client_ip::ClientIp;
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Query};
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
        .routes(routes!(pending))
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
    Ok(Json(service::start(&s, req, ip.0).await?))
}

/// What a pending login is asking for: host name, start time, coarse address (session only).
#[utoipa::path(get, path = "/auth/cli/pending", operation_id = "getPendingCliLogin", tag = "auth",
    params(PendingQuery),
    responses((status = 200, body = PendingResponse), (status = 400, body = ErrorEnvelope),
        (status = 404, body = ErrorEnvelope), (status = 429, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn pending(
    c: SessionCaller,
    State(s): State<AppState>,
    Query(q): Query<PendingQuery>,
) -> Result<Json<PendingResponse>, ApiError> {
    Ok(Json(service::pending(&s, &c, &q.code).await?))
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
