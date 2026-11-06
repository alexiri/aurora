//! Error plumbing. Handlers return `Result<T, ApiError>`; `From<DbError>` lives in
//! `aurora_db` (`aurora_db::conflict_code` maps unique violations to domain codes).

use aurora_core::ApiError;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;

pub type ApiResult<T> = Result<T, ApiError>;

/// OpenAPI description of the error envelope (`02 §Error envelope`).
#[derive(Serialize, ToSchema)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

#[derive(Serialize, ToSchema)]
pub struct ErrorBody {
    /// snake_case machine code.
    pub code: String,
    pub message: String,
    /// Optional; validation errors carry `{"fields":[{"path","message"}]}`.
    #[schema(value_type = Option<Object>)]
    pub details: Option<serde_json::Value>,
}

pub async fn not_found_fallback() -> Response {
    ApiError::not_found("No such route.").into_response()
}

pub async fn method_not_allowed_fallback() -> Response {
    let mut r = ApiError::new(
        http::StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        "Method not allowed.",
    )
    .into_response();
    *r.status_mut() = http::StatusCode::METHOD_NOT_ALLOWED;
    r
}
