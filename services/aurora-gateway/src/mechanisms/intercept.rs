//! Intercept stage: `oauth_token_endpoint` (docs/09 §5). SDK refresh calls are answered
//! locally with the connection's current access token; nothing goes upstream.

use crate::{Body, full_body};
use aurora_registry::manifest::Intercept;
use http::{Method, Response, StatusCode, header};

pub fn matches(i: &Intercept, method: &Method, path: &str) -> bool {
    match i {
        Intercept::OauthTokenEndpoint(e) => {
            e.path == path && method.as_str().eq_ignore_ascii_case(&e.method)
        }
    }
}

pub fn token_response(access_token: &str, expires_in: i64) -> Response<Body> {
    let body = serde_json::json!({
        "access_token": access_token,
        "token_type": "Bearer",
        "expires_in": expires_in.max(0),
    });
    let mut r = Response::new(full_body(body.to_string()));
    *r.status_mut() = StatusCode::OK;
    let h = r.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    h.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    r
}
