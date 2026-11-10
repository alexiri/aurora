//! HTTP error envelope: `{"error":{"code","message","details"?}}`.

use http::StatusCode;
use serde::Serialize;
use serde_json::{Value, json};
use std::fmt;

/// Maximum length of an error `code`.
pub const CODE_MAX_LEN: usize = 64;
/// Code substituted when a caller supplies an invalid one (never panics on input).
pub const FALLBACK_CODE: &str = "internal_error";

/// A single validation failure (`details.fields[]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldError {
    /// JSON pointer to the offending field, e.g. `/name`.
    pub path: String,
    pub message: String,
}

impl FieldError {
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("error code must be 1-64 chars of [a-z0-9_]")]
pub struct InvalidCode;

/// `true` if `code` is 1..=64 chars of `[a-z0-9_]`.
pub fn is_valid_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= CODE_MAX_LEN
        && code
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{status} {code}: {message}")]
pub struct ApiError {
    status: StatusCode,
    code: String,
    message: String,
    details: Option<Value>,
    retry_after_secs: Option<u64>,
}

#[derive(Serialize)]
struct Envelope<'a> {
    error: Body<'a>,
}

#[derive(Serialize)]
struct Body<'a> {
    code: &'a str,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<&'a Value>,
}

impl Serialize for ApiError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        Envelope {
            error: Body {
                code: &self.code,
                message: &self.message,
                details: self.details.as_ref(),
            },
        }
        .serialize(s)
    }
}

impl ApiError {
    /// Validated constructor.
    pub fn try_new(
        status: StatusCode,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Result<Self, InvalidCode> {
        let code = code.into();
        if !is_valid_code(&code) {
            return Err(InvalidCode);
        }
        Ok(Self {
            status,
            code,
            message: message.into(),
            details: None,
            retry_after_secs: None,
        })
    }

    /// Like [`try_new`](Self::try_new), but an invalid code is replaced by
    /// [`FALLBACK_CODE`] (a programming error must not become a panic).
    pub fn new(status: StatusCode, code: impl Into<String>, message: impl Into<String>) -> Self {
        let code = code.into();
        let code = if is_valid_code(&code) {
            code
        } else {
            FALLBACK_CODE.to_owned()
        };
        Self {
            status,
            code,
            message: message.into(),
            details: None,
            retry_after_secs: None,
        }
    }

    #[must_use]
    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }
    pub fn code(&self) -> &str {
        &self.code
    }
    pub fn message(&self) -> &str {
        &self.message
    }
    pub fn details(&self) -> Option<&Value> {
        self.details.as_ref()
    }
    pub fn retry_after_secs(&self) -> Option<u64> {
        self.retry_after_secs
    }

    /// (P4-CNT, D-119) 503 `counter_unavailable` with `retry-after: 5`: a `closed` counter class
    /// refused because the shared counter backend is unreachable. Never a 429.
    pub fn counter_unavailable(limiter: &str) -> Self {
        let mut e = Self::unavailable(
            "counter_unavailable",
            "The rate-limit counters are unavailable, so this request was refused. Retry shortly.",
        )
        .with_details(serde_json::json!({ "limiter": limiter }));
        e.retry_after_secs = Some(5);
        e
    }

    /// 400 `invalid_request` with `details.fields = [{path, message}]`.
    pub fn invalid_request(message: impl Into<String>, fields: Vec<FieldError>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
            .with_details(json!({ "fields": fields }))
    }

    pub fn unauthenticated(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthenticated", message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    pub fn conflict(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }

    /// 410 retired.
    pub fn gone(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::GONE, code, message)
    }

    pub fn too_large(message: impl Into<String>) -> Self {
        Self::new(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large", message)
    }

    /// 422 domain-rule violation with a caller-chosen code.
    pub fn unprocessable(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, code, message)
    }

    /// 429 with `retry-after` (seconds).
    pub fn rate_limited(retry_after_secs: u64, message: impl Into<String>) -> Self {
        let mut e = Self::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", message);
        e.retry_after_secs = Some(retry_after_secs);
        e
    }

    /// 502.
    pub fn bad_gateway(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, code, message)
    }

    /// 503.
    pub fn unavailable(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, code, message)
    }

    /// 504.
    pub fn gateway_timeout(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::GATEWAY_TIMEOUT, code, message)
    }

    /// 500; keep the message generic (never leak internals).
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
    }

    /// Serialized body bytes.
    pub fn to_json_vec(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_else(|_| {
            br#"{"error":{"code":"internal_error","message":"serialization failed"}}"#.to_vec()
        })
    }
}

impl fmt::Display for FieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

#[cfg(feature = "axum")]
impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        use http::{HeaderValue, header};
        let body = self.to_json_vec();
        let mut res = axum::response::Response::new(axum::body::Body::from(body));
        *res.status_mut() = self.status;
        res.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        if let Some(secs) = self.retry_after_secs {
            res.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(secs));
        }
        res
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn j(e: &ApiError) -> Value {
        serde_json::to_value(e).unwrap()
    }

    #[test]
    fn without_details() {
        let e = ApiError::not_found("No such agent.");
        assert_eq!(e.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            j(&e),
            json!({"error": {"code": "not_found", "message": "No such agent."}})
        );
    }

    #[test]
    fn with_details() {
        let e = ApiError::conflict("handle_taken", "Taken.").with_details(json!({"handle": "x"}));
        assert_eq!(
            j(&e),
            json!({"error": {"code": "handle_taken", "message": "Taken.", "details": {"handle": "x"}}})
        );
    }

    #[test]
    fn invalid_request_fields() {
        let e = ApiError::invalid_request("Bad.", vec![FieldError::new("/name", "too long")]);
        assert_eq!(e.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            j(&e),
            json!({"error": {"code": "invalid_request", "message": "Bad.",
                "details": {"fields": [{"path": "/name", "message": "too long"}]}}})
        );
    }

    #[test]
    fn status_vocabulary() {
        assert_eq!(ApiError::unauthenticated("x").status(), 401);
        assert_eq!(ApiError::forbidden("x").status(), 403);
        assert_eq!(ApiError::gone("agent_retired", "x").status(), 410);
        assert_eq!(ApiError::too_large("x").status(), 413);
        assert_eq!(ApiError::unprocessable("rule", "x").status(), 422);
        assert_eq!(ApiError::bad_gateway("up", "x").status(), 502);
        assert_eq!(ApiError::unavailable("up", "x").status(), 503);
        assert_eq!(ApiError::gateway_timeout("up", "x").status(), 504);
        let r = ApiError::rate_limited(7, "slow");
        assert_eq!((r.status().as_u16(), r.retry_after_secs()), (429, Some(7)));
    }

    #[test]
    fn code_validation() {
        assert!(is_valid_code("a_1"));
        assert!(is_valid_code(&"a".repeat(64)));
        assert!(!is_valid_code(&"a".repeat(65)));
        for bad in ["", "A", "a-b", "a b", "é"] {
            assert!(!is_valid_code(bad), "{bad}");
            assert_eq!(
                ApiError::try_new(StatusCode::CONFLICT, bad, "m"),
                Err(InvalidCode)
            );
            assert_eq!(ApiError::conflict(bad, "m").code(), FALLBACK_CODE);
        }
    }

    #[cfg(feature = "axum")]
    #[test]
    fn axum_response() {
        use axum::response::IntoResponse;
        let res = ApiError::rate_limited(30, "slow").into_response();
        assert_eq!(res.status(), 429);
        assert_eq!(res.headers()["retry-after"], "30");
        assert_eq!(res.headers()["content-type"], "application/json");
        let res = ApiError::not_found("x").into_response();
        assert!(res.headers().get("retry-after").is_none());
    }
}
