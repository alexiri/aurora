//! `GET /v1/agents/{agent}/terminal` (P4-TERM, C-616): the web terminal's WebSocket upgrade.
//! Session cookie only (API keys, agent tokens and machine callers are refused 401), same
//! origin (`Origin` checked), 404 `terminal_disabled` while the feature is off. Excluded from
//! the OpenAPI document (a WebSocket upgrade is not a JSON API).
use crate::app::AppState;
use crate::domains::terminal::service;
use crate::http::auth::Caller;
use aurora_core::ApiError;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::response::Response;
use serde::Deserialize;
use uuid::Uuid;

pub fn router() -> utoipa_axum::router::OpenApiRouter<AppState> {
    utoipa_axum::router::OpenApiRouter::new()
        .route("/agents/{agent}/terminal", axum::routing::get(terminal))
}

#[derive(Debug, Deserialize)]
struct TerminalQuery {
    /// Long-poll ≤ `TERMINAL_WAKE_WAIT` before answering (the dashboard's default is to take
    /// the `202 {waking: true}` and retry on ready).
    wait: Option<bool>,
}

/// The upgrade itself: admission (C-616's matrix) happens before the upgrade so every refusal
/// is a normal JSON error; only success upgrades to 101 and becomes a byte bridge.
async fn terminal(
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    Query(q): Query<TerminalQuery>,
    mut parts: Parts,
) -> Result<Response, ApiError> {
    // 404 `terminal_disabled` while the feature is off (C-616) — before any auth work.
    if !s.config.terminal.enabled {
        return Err(ApiError::not_found("This instance has no web terminal."));
    }
    // Session cookie only: API keys, agent tokens and anonymous callers are refused 401
    // (SEC-87; the `Caller` extractor already refuses agent tokens, this narrows to sessions).
    let caller = <Caller as FromRequestParts<AppState>>::from_request_parts(&mut parts, &s).await?;
    if !caller.is_session() {
        return Err(ApiError::unauthenticated(
            "The web terminal needs a dashboard session.",
        ));
    }
    // Same origin (C-616: `Origin` checked, cross-origin upgrade refused).
    let origin = parts
        .headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .and_then(crate::http::csrf::origin_of);
    match origin {
        Some(o) if s.net.allowed_origins.contains(&o) => {}
        _ => {
            return Err(ApiError::new(
                axum::http::StatusCode::FORBIDDEN,
                "csrf_rejected",
                "Cross-origin request rejected.",
            ));
        }
    }
    let wait = q.wait.unwrap_or(false);
    // The upgrade must be requested here (extractor in this handler) and consumed by the
    // service on success only.
    let ws = WebSocketUpgrade::from_request_parts(&mut parts, &s)
        .await
        .map_err(|_| {
            ApiError::new(
                axum::http::StatusCode::BAD_REQUEST,
                "bad_request",
                "A WebSocket upgrade is required.",
            )
        })?;
    service::upgrade(s, caller, agent, ws, wait).await
}
