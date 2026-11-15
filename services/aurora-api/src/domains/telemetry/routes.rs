use super::dto::OtlpExportRequest;
use super::sanitize;
use crate::app::AppState;
use crate::http::auth::SessionCaller;
use crate::http::error::ErrorEnvelope;
use aurora_core::limits::TELEMETRY_INGEST_BODY_MAX;
use aurora_core::{ApiError, FieldError};
use axum::body::Body;
use axum::extract::{FromRequestParts, State};
use axum::http::{StatusCode, header, request::Parts};
use std::sync::Arc;
use std::time::SystemTime;
use utoipa_axum::{router::OpenApiRouter, routes};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(ingest_traces))
}

/// Resolves first, before authentication: without an OTLP endpoint the route does not exist (404,
/// like any unknown route), so a deployment that exports nothing exposes no ingestion surface.
struct Enabled(Arc<super::BrowserSpans>);

impl FromRequestParts<AppState> for Enabled {
    type Rejection = ApiError;

    async fn from_request_parts(_: &mut Parts, s: &AppState) -> Result<Self, ApiError> {
        s.browser_spans
            .clone()
            .map(Self)
            .ok_or_else(|| ApiError::not_found("Not found."))
    }
}

/// Browser span ingestion (the dashboard's traces). OTLP/HTTP JSON, at most 256 KiB, 512 spans,
/// 32 attributes and 8 events per span. The server rebuilds every span (resource, name, allow-listed
/// attributes, clamped times); a span failing the sampling rule is dropped. 204 even when every span
/// was dropped. 404 when the deployment exports no traces.
#[utoipa::path(post, path = "/telemetry/traces", operation_id = "ingestBrowserTraces", tag = "telemetry",
    request_body(content = OtlpExportRequest, content_type = "application/json"),
    responses((status = 204, description = "Accepted"), (status = 400, body = ErrorEnvelope),
        (status = 401, body = ErrorEnvelope), (status = 404, body = ErrorEnvelope),
        (status = 413, body = ErrorEnvelope), (status = 429, body = ErrorEnvelope)),
    security(("session" = [])))]
async fn ingest_traces(
    Enabled(spans): Enabled,
    c: SessionCaller,
    State(_s): State<AppState>,
    headers: axum::http::HeaderMap,
    body: Body,
) -> Result<StatusCode, ApiError> {
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("application/json")
        });
    if !is_json {
        return Err(ApiError::invalid_request(
            "Content-Type must be application/json.",
            vec![FieldError::new("", "expected application/json")],
        ));
    }
    let too_large = || {
        ApiError::too_large(format!(
            "The request is larger than {} KiB.",
            TELEMETRY_INGEST_BODY_MAX / 1024
        ))
    };
    if headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        .is_some_and(|n| n > TELEMETRY_INGEST_BODY_MAX)
    {
        return Err(too_large());
    }
    let bytes = axum::body::to_bytes(body, TELEMETRY_INGEST_BODY_MAX)
        .await
        .map_err(|_| too_large())?;
    let req: OtlpExportRequest = serde_json::from_slice(&bytes).map_err(|e| {
        ApiError::invalid_request(
            "The request is not a valid OTLP/HTTP JSON trace export.",
            vec![FieldError::new("", e.to_string())],
        )
    })?;
    let rebuilt = sanitize::rebuild(&req, SystemTime::now(), spans.threshold)
        .map_err(|f| ApiError::invalid_request("Invalid trace export.", vec![f]))?;
    if !rebuilt.spans.is_empty() {
        (spans.sink)(sanitize::resource(c.user_id), rebuilt.spans);
    }
    Ok(StatusCode::NO_CONTENT)
}
