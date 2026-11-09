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

/// (phase 2, RFE-006) 403 `frozen`: the credential is fine, the agent (or its workspace or
/// organization) is paused by an administrator. Not 407: the client must not re-authenticate.
pub fn frozen(level: &str) -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "frozen",
        "This agent is paused by an administrator. Its credentials are intact and work again as soon as it is unfrozen.",
    )
    .with_details(json!({ "level": level }))
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

/// (RFE-003) The credential does not hold the `browser` capability.
pub fn open_lane_not_permitted() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "open_lane_not_permitted",
        "This credential may not use the open (tunnel) lane. Browser tunnels are available only to sandboxes started with the browser capability; use the default proxy lane.",
    )
}

/// (RFE-003) The effective open-lane posture is `disabled`.
pub fn open_lane_disabled(manage_url: &str) -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "open_lane_disabled",
        format!(
            "Browser tunnels are disabled by policy. Review the open-lane setting at {manage_url}"
        ),
    )
    .with_details(json!({ "manage_url": manage_url }))
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

/// (phase 3, RFE-014, C-382) 503 `gateway_degraded`: the database is unreachable and the request
/// needs state the caches do not hold. `reason` is `database_unavailable`,
/// `credential_store_unavailable` or `snapshot_not_loaded`. Never a 407: the client must not
/// conclude that its token is bad. Counted in `aurora_gateway_degraded_requests_total`.
pub fn gateway_degraded(reason: &'static str) -> ApiError {
    crate::health::count_refused(reason);
    ApiError::unavailable(
        "gateway_degraded",
        "The gateway cannot reach its database and this request needs information it does not hold. Retry shortly.",
    )
    .with_details(json!({ "reason": reason }))
}

/// Codes answered with `retry-after: 5` and `x-should-retry: true` (C-382).
fn is_degraded_refusal(err: &ApiError) -> bool {
    err.status() == StatusCode::SERVICE_UNAVAILABLE
        && matches!(
            err.code(),
            "gateway_degraded" | "approval_store_unavailable"
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
    if is_degraded_refusal(err) {
        h.insert("x-should-retry", HeaderValue::from_static("true"));
        h.insert(
            header::RETRY_AFTER,
            HeaderValue::from(crate::health::RETRY_AFTER_SECS),
        );
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

/// 503 `gateway_restarting` for a new `CONNECT` while the gateway drains (C-382).
pub fn restarting_error() -> ApiError {
    ApiError::unavailable(
        "gateway_restarting",
        "The gateway is restarting. Retry shortly.",
    )
}

/// 503 `gateway_restarting` with `retry-after: 1` (held request at SIGTERM).
pub fn restarting() -> Response<Body> {
    let err = ApiError::unavailable(
        "gateway_restarting",
        "The gateway is restarting while this request awaited approval. Retry shortly.",
    );
    let mut res = response(&err);
    res.headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(1u64));
    res
}
