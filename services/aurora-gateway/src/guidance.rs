//! Agent-facing refusal envelopes (06 §Guidance responses).

use crate::{Body, full_body};
use aurora_core::ApiError;
use http::{HeaderValue, Response, StatusCode, header};
use serde_json::json;

pub fn proxy_auth_required() -> ApiError {
    ApiError::new(
        StatusCode::PROXY_AUTHENTICATION_REQUIRED,
        "proxy_auth_required",
        "This proxy requires a valid Aurora gateway credential in Proxy-Authorization (Basic, password = the credential token).",
    )
}

pub fn not_a_proxy_request() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "not_a_proxy_request",
        "This port is a forward proxy. Send CONNECT host:port or an absolute-form http:// request.",
    )
}

pub fn destination_refused(host: &str) -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "destination_refused",
        "The destination is a private or internal address and is not permitted.",
    )
    .with_details(json!({ "host": host }))
}

pub fn method_override_refused() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "method_override_refused",
        "Method override headers and the _method query parameter are not allowed; send the real HTTP method.",
    )
}

pub fn ambiguous_path() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "ambiguous_path",
        "The request path contains dot segments, encoded slashes, backslashes or NUL and cannot be interpreted unambiguously.",
    )
}

pub fn upstream_timeout() -> ApiError {
    ApiError::gateway_timeout("upstream_timeout", "The upstream did not respond in time.")
}

pub fn bad_gateway() -> ApiError {
    ApiError::bad_gateway("bad_gateway", "Could not connect to the upstream.")
}

pub fn resolution_failed() -> ApiError {
    ApiError::bad_gateway("resolution_failed", "Could not resolve the upstream host.")
}

pub fn credential_store_unavailable() -> ApiError {
    ApiError::unavailable(
        "credential_store_unavailable",
        "Credentials cannot be verified right now. Retry shortly.",
    )
}

/// Renders an envelope. Definite (4xx) verdicts carry `x-should-retry: false`.
pub fn response(err: &ApiError) -> Response<Body> {
    let mut res = Response::new(full_body(err.to_json_vec()));
    *res.status_mut() = err.status();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    if err.status().is_client_error() {
        h.insert("x-should-retry", HeaderValue::from_static("false"));
    }
    if let Some(s) = err.retry_after_secs() {
        h.insert(header::RETRY_AFTER, HeaderValue::from(s));
    }
    if err.status() == StatusCode::PROXY_AUTHENTICATION_REQUIRED {
        h.insert(
            header::PROXY_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"aurora\""),
        );
    }
    res
}
