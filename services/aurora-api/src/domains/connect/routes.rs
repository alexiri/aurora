use super::dto::{ConnectRequest, ConnectResponse};
use super::{flow, page, service};
use crate::app::AppState;
use crate::http::auth::{Caller, SessionCaller, cookie_value};
use crate::http::error::ErrorEnvelope;
use crate::http::json::{Json, Path};
use aurora_core::ApiError;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use http::{HeaderMap, HeaderValue, StatusCode, header};
use std::collections::HashMap;
use utoipa_axum::{router::OpenApiRouter, routes};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(connect))
        .routes(routes!(callback))
}

/// Start or complete a connection (fields/hook) or begin an OAuth redirect.
#[utoipa::path(
    post, path = "/apps/{app}/connect", operation_id = "connectApp", tag = "connections",
    params(("app" = String, Path, description = "App id")),
    request_body = ConnectRequest,
    responses((status = 200, description = "Redirect or connected", body = ConnectResponse),
              (status = 400, description = "invalid_request", body = ErrorEnvelope),
              (status = 403, description = "app_unavailable / forbidden", body = ErrorEnvelope),
              (status = 404, description = "Not found", body = ErrorEnvelope),
              (status = 409, description = "conflict", body = ErrorEnvelope),
              (status = 422, description = "connect_probe_failed | oauth_client_required | connect_hook_failed", body = ErrorEnvelope)),
    security(("session" = []))
)]
async fn connect(
    caller: SessionCaller,
    State(state): State<AppState>,
    Path(app): Path<String>,
    Json(req): Json<ConnectRequest>,
) -> Result<Response, ApiError> {
    let started = service::start(&state, &caller.0, &app, req).await?;
    let mut r = Json(started.response).into_response();
    if let Some(c) = started.cookie
        && let Ok(v) = HeaderValue::from_str(&c)
    {
        r.headers_mut().append(header::SET_COOKIE, v);
    }
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(r)
}

/// OAuth callback: HTML page posting `aurora:connected` to the opener, or `error=<code>`.
#[utoipa::path(
    get, path = "/oauth/callback", operation_id = "oauthCallback", tag = "connections",
    responses((status = 200, description = "HTML page", content_type = "text/html"),
              (status = 302, description = "Continue redirect (hook methods)")),
)]
async fn callback(
    caller: Result<Caller, ApiError>,
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let cookie = cookie_value(&headers, flow::COOKIE);
    let Ok(caller) = caller else {
        return page::error("oauth_state_invalid");
    };
    // Fragment-mode providers: no query yet, the bridge re-submits the fragment.
    if q.is_empty() && cookie.is_some() {
        return page::bridge();
    }
    match service::callback(&state, &caller, &q, cookie).await {
        Ok(service::Done::Connected(id, missing)) => {
            let origin =
                crate::http::csrf::origin_of(&state.config.external_url).unwrap_or_default();
            page::connected(id, &missing, &origin)
        }
        Ok(service::Done::Redirect { url, cookie }) => {
            let mut r = StatusCode::FOUND.into_response();
            let h = r.headers_mut();
            if let Ok(v) = HeaderValue::from_str(&url) {
                h.insert(header::LOCATION, v);
            }
            if let Ok(v) =
                HeaderValue::from_str(&flow::cookie(&cookie_val(&cookie), flow::cookie_ttl()))
            {
                h.append(header::SET_COOKIE, v);
            }
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            r
        }
        Err(code) => page::error(code),
    }
}

/// `Done::Redirect.cookie` already is a full `Set-Cookie` value.
fn cookie_val(full: &str) -> String {
    full.split_once('=')
        .and_then(|(_, rest)| rest.split(';').next())
        .unwrap_or_default()
        .to_owned()
}
