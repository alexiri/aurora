use super::auth::{RunnerCtx, RunnerToken};
use super::service;
use crate::app::AppState;
use crate::http::json::Json;
use crate::http::json::Path;
use aurora_core::ApiError;
use aurora_protocol::common::Validate;
use aurora_protocol::runner::*;
use axum::body::Body;
use axum::extract::State;
use axum::response::Response;
use axum::routing::{get, post};
use http::HeaderMap;
use utoipa_axum::router::OpenApiRouter;
use uuid::Uuid;

/// Mounted under `/v1`; plain routes, so excluded from the dashboard OpenAPI document.
pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .route("/runner/register", post(register))
        .route("/runner/work", post(work))
        .route("/runner/reports", post(reports))
        .route("/runner/channel/claim", post(claim))
        .route("/runner/channel/hold", post(hold))
        .route("/runner/channel/release", post(release))
        .route("/runner/tool-calls", post(tool_calls))
        .route("/runner/memory-writes", post(memory_writes))
        .route("/runner/attachments", post(attachment_upload))
        .route("/runner/attachments/{id}", get(attachment_download))
        .route("/runner/sandboxes", get(sandboxes))
        .route("/runner/sandboxes/check", post(check))
        .route("/runner/strays", post(strays))
        .route("/runner/retire", post(retire))
}

async fn register(
    t: RunnerToken,
    State(s): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>, ApiError> {
    Ok(Json(service::register(&s, &t.0, req).await?))
}

async fn work(
    c: RunnerCtx,
    State(s): State<AppState>,
    Json(req): Json<WorkRequest>,
) -> Result<Json<WorkResponse>, ApiError> {
    req.validate()
        .map_err(|e| ApiError::invalid_request(e.to_string(), vec![]))?;
    Ok(Json(
        service::poll(&s, &c.runner, c.replica, req.wait_secs, req.capacity).await?,
    ))
}

async fn reports(
    c: RunnerCtx,
    State(s): State<AppState>,
    Json(req): Json<ReportsRequest>,
) -> Result<Json<ReportsResponse>, ApiError> {
    Ok(Json(service::reports(&s, &c.runner, c.replica, req).await?))
}

async fn claim(
    c: RunnerCtx,
    State(s): State<AppState>,
    Json(req): Json<ChannelClaimRequest>,
) -> Result<Json<ChannelClaimResponse>, ApiError> {
    Ok(Json(service::channel_claim(&s, &c.runner, req).await?))
}

async fn hold(
    c: RunnerCtx,
    State(s): State<AppState>,
    Json(req): Json<ChannelRef>,
) -> Result<Json<OkResponse>, ApiError> {
    service::channel_hold(&s, &c.runner, c.replica, req).await?;
    Ok(Json(OkResponse { ok: true }))
}

async fn release(
    c: RunnerCtx,
    State(s): State<AppState>,
    Json(req): Json<ChannelRef>,
) -> Result<Json<OkResponse>, ApiError> {
    service::channel_release(&s, &c.runner, c.replica, req).await?;
    Ok(Json(OkResponse { ok: true }))
}

async fn tool_calls(
    c: RunnerCtx,
    State(s): State<AppState>,
    Json(req): Json<aurora_protocol::tools::ToolCall>,
) -> Result<Json<aurora_protocol::tools::ToolResult>, ApiError> {
    Ok(Json(
        crate::domains::tools::service::tool_call(&s, &c.runner, req).await?,
    ))
}

async fn memory_writes(
    c: RunnerCtx,
    State(s): State<AppState>,
    Json(req): Json<MemoryWriteRequest>,
) -> Result<Json<MemoryWriteResponse>, ApiError> {
    Ok(Json(
        crate::domains::memory::memory_write(&s, &c.runner, req).await?,
    ))
}

async fn attachment_upload(
    c: RunnerCtx,
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<AttachmentUploadResponse>, ApiError> {
    Ok(Json(
        crate::domains::attachments::runner::upload(&s, &c.runner, &headers, body).await?,
    ))
}

async fn attachment_download(
    c: RunnerCtx,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    crate::domains::attachments::runner::download(&s, &c.runner, id).await
}

async fn sandboxes(
    c: RunnerCtx,
    State(s): State<AppState>,
) -> Result<Json<SandboxesResponse>, ApiError> {
    Ok(Json(service::sandboxes_list(&s, &c.runner).await?))
}

async fn check(
    _c: RunnerCtx,
    State(s): State<AppState>,
    Json(req): Json<SandboxesCheckRequest>,
) -> Result<Json<SandboxesCheckResponse>, ApiError> {
    Ok(Json(service::sandboxes_check(&s, req).await?))
}

async fn strays(
    c: RunnerCtx,
    Json(req): Json<StraysRequest>,
) -> Result<Json<OkResponse>, ApiError> {
    service::strays(&c.runner, &req);
    Ok(Json(OkResponse { ok: true }))
}

async fn retire(c: RunnerCtx, State(s): State<AppState>) -> Result<Json<OkResponse>, ApiError> {
    service::retire(&s, &c.runner, c.replica).await?;
    Ok(Json(OkResponse { ok: true }))
}
