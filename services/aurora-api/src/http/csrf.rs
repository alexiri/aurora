//! CSRF defence for cookie-authenticated unsafe requests (docs/04 §Sessions, SEC-22).
//! Requests carrying `Authorization: Bearer` are exempt (key auth never reads cookies).
//! While previews are enabled (P4-PRV-2, `04 §Preview access` rule 5) a cookie-authenticated
//! request with `Sec-Fetch-Site: same-site|cross-site` that is not a top-level `GET` navigation
//! is refused 403 `csrf_rejected` — a preview origin can never act with the dashboard session.

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

/// The Fetch-Metadata check (`04 §Preview access` rule 5, SEC-81): while previews are on,
/// a cookie-bearing request whose `Sec-Fetch-Site` is `same-site` or `cross-site` is refused
/// unless it is a top-level `GET` navigation (`Sec-Fetch-Mode: navigate` from a top-level
/// document — `Sec-Fetch-Dest: document`, no `Sec-Fetch-User` requirement). Requests without
/// the header (old browsers, curl) are not refused by this rule; the Origin check above and
/// SameSite=Lax still apply.
fn fetch_metadata_ok(headers: &http::HeaderMap, method: &Method) -> bool {
    let site = headers
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !matches!(site, "same-site" | "cross-site") {
        return true;
    }
    // A top-level navigation is a GET (the middleware only calls here for unsafe methods, so
    // any unsafe same-site/cross-site request is refused; a safe one passes the method gate).
    let mode = headers
        .get("sec-fetch-mode")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let dest = headers
        .get("sec-fetch-dest")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    *method == Method::GET && mode == "navigate" && dest == "document"
}

pub async fn middleware(State(state): State<AppState>, req: Request, next: Next) -> Response {
    // Only `/v1/**` reads the session cookie (the SCIM plane ignores cookies and `Origin`
    // entirely, 04 §SCIM: a cookie-only request there is answered by its own 401). The
    // plane's paths live under `/scim/v2`; everything else (test routes, ops) keeps the
    // check so the middleware is path-scoped, not method-scoped.
    let cookie_req = (req.uri().path().starts_with("/v1/")
        || !req.uri().path().starts_with("/scim/"))
        && is_unsafe(req.method())
        && !req.headers().contains_key(header::AUTHORIZATION)
        && cookie_value(req.headers(), SESSION_COOKIE).is_some();
    if cookie_req {
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
    // (P4-PRV-2) The Fetch-Metadata rule applies to every method while previews are on.
    if state.config.previews.enabled
        && (req.uri().path().starts_with("/v1/") || !req.uri().path().starts_with("/scim/"))
        && !req.headers().contains_key(header::AUTHORIZATION)
        && cookie_value(req.headers(), SESSION_COOKIE).is_some()
        && !fetch_metadata_ok(req.headers(), req.method())
    {
        return ApiError::new(
            http::StatusCode::FORBIDDEN,
            "csrf_rejected",
            "Cross-site request rejected.",
        )
        .into_response();
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

    #[test]
    fn fetch_metadata_matrix() {
        let h = |site: &str, mode: &str, dest: &str| {
            let mut m = http::HeaderMap::new();
            m.insert("sec-fetch-site", site.parse().unwrap());
            m.insert("sec-fetch-mode", mode.parse().unwrap());
            m.insert("sec-fetch-dest", dest.parse().unwrap());
            m
        };
        // Navigations pass; every other same-site/cross-site mode is refused.
        assert!(fetch_metadata_ok(
            &h("same-site", "navigate", "document"),
            &Method::GET
        ));
        assert!(fetch_metadata_ok(
            &h("cross-site", "navigate", "document"),
            &Method::GET
        ));
        assert!(!fetch_metadata_ok(
            &h("same-site", "cors", "empty"),
            &Method::POST
        ));
        assert!(!fetch_metadata_ok(
            &h("cross-site", "cors", "empty"),
            &Method::GET
        ));
        assert!(!fetch_metadata_ok(
            &h("same-site", "no-cors", "script"),
            &Method::GET
        ));
        assert!(!fetch_metadata_ok(
            &h("same-site", "navigate", "iframe"),
            &Method::GET
        ));
        // `same-origin` is the dashboard itself; the Origin check governs it, not this rule.
        assert!(fetch_metadata_ok(
            &h("same-origin", "cors", "empty"),
            &Method::POST
        ));
        // No header at all: the rule does not fire (SameSite and the Origin check apply).
        assert!(fetch_metadata_ok(&http::HeaderMap::new(), &Method::POST));
    }
}
