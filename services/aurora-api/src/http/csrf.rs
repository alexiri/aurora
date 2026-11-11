//! CSRF defence for cookie-authenticated unsafe requests (docs/04 §Sessions, SEC-22).
//! Requests carrying `Authorization: Bearer` are exempt (key auth never reads cookies).

use crate::app::AppState;
use crate::http::auth::{SESSION_COOKIE, cookie_value};
use aurora_core::ApiError;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::{Method, header};

/// `scheme://host[:port]`, lower-cased, no trailing slash; `None` if not an origin-like URL.
pub fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if scheme.is_empty() || authority.is_empty() {
        return None;
    }
    Some(format!(
        "{}://{}",
        scheme.to_ascii_lowercase(),
        authority.to_ascii_lowercase()
    ))
}

fn is_unsafe(m: &Method) -> bool {
    !matches!(
        *m,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    )
}

pub async fn middleware(State(state): State<AppState>, req: Request, next: Next) -> Response {
    // Only `/v1/**` reads the session cookie (the SCIM plane ignores cookies and `Origin`
    // entirely, 04 §SCIM: a cookie-only request there is answered by its own 401). The
    // plane's paths live under `/scim/v2`; everything else (test routes, ops) keeps the
    // check so the middleware is path-scoped, not method-scoped.
    if (req.uri().path().starts_with("/v1/") || !req.uri().path().starts_with("/scim/"))
        && is_unsafe(req.method())
        && !req.headers().contains_key(header::AUTHORIZATION)
        && cookie_value(req.headers(), SESSION_COOKIE).is_some()
    {
        let ok = req
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            .and_then(origin_of)
            .is_some_and(|o| state.net.allowed_origins.contains(&o));
        if !ok {
            return ApiError::new(
                http::StatusCode::FORBIDDEN,
                "csrf_rejected",
                "Cross-origin request rejected.",
            )
            .into_response();
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins() {
        assert_eq!(
            origin_of("http://Localhost:4101/").as_deref(),
            Some("http://localhost:4101")
        );
        assert_eq!(origin_of("https://a.b/x/y").as_deref(), Some("https://a.b"));
        assert_eq!(origin_of("null"), None);
    }

    #[test]
    fn unsafe_paths() {
        // (P4-GRP-2) The CSRF check itself is path-scoped in the middleware; this keeps the
        // method table pinned.
        assert!(is_unsafe(&http::Method::POST));
        assert!(!is_unsafe(&http::Method::GET));
    }
}
