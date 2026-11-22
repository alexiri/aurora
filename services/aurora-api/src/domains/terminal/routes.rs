//! `GET /v1/agents/{agent}/terminal` (P4-TERM, C-616): the web terminal's WebSocket upgrade.
//! Session cookie only (API keys, agent tokens and machine callers are refused 401), same
//! origin (`Origin` checked), 404 `terminal_disabled` while the feature is off. Excluded from
//! the OpenAPI document (a WebSocket upgrade is not a JSON API).
use crate::app::AppState;
use crate::domains::terminal::service;
use crate::http::auth::Caller;
use aurora_core::ApiError;
use aurora_protocol::runner::PtyShell;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::response::Response;
use serde::Deserialize;
use uuid::Uuid;

pub fn router() -> utoipa_axum::router::OpenApiRouter<AppState> {
    utoipa_axum::router::OpenApiRouter::new()
        .route("/agents/{agent}/terminal", axum::routing::get(terminal))
        .routes(utoipa_axum::routes!(get_shell, put_shell))
}

/// (FIX-218, D-459, C-1145) The shell preference of the signed-in person for an agent.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct TerminalShellPreference {
    /// `bash` or `zsh`; `bash` when nothing is saved.
    #[schema(example = "bash")]
    pub shell: String,
    /// Whether a choice is saved (`false` = the default applies).
    pub stored: bool,
}

/// Body of `PUT /agents/{agent}/terminal/shell`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PutTerminalShell {
    /// `bash` or `zsh`; anything else is 422 `invalid_shell`.
    pub shell: String,
}

/// The person's chosen shell for this agent: used by the web terminal (when `?shell=` is
/// absent) and by SSH. Session cookie, `can_use`, hosted agent; frozen agents still answer.
#[utoipa::path(get, path = "/agents/{agent}/terminal/shell", operation_id = "getTerminalShell",
    tag = "terminal", params(("agent" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = TerminalShellPreference),
        (status = 401, body = crate::http::error::ErrorEnvelope),
        (status = 403, body = crate::http::error::ErrorEnvelope),
        (status = 404, body = crate::http::error::ErrorEnvelope),
        (status = 422, body = crate::http::error::ErrorEnvelope)),
    security(("session" = [])))]
async fn get_shell(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
) -> Result<axum::Json<TerminalShellPreference>, ApiError> {
    Ok(axum::Json(
        service::get_shell_preference(&s, &c, agent).await?,
    ))
}

/// Saves the person's shell choice for this agent (`bash` or `zsh`).
#[utoipa::path(put, path = "/agents/{agent}/terminal/shell", operation_id = "putTerminalShell",
    tag = "terminal", params(("agent" = Uuid, Path, description = "Agent id")),
    request_body = PutTerminalShell,
    responses((status = 200, body = TerminalShellPreference),
        (status = 401, body = crate::http::error::ErrorEnvelope),
        (status = 403, body = crate::http::error::ErrorEnvelope),
        (status = 404, body = crate::http::error::ErrorEnvelope),
        (status = 422, body = crate::http::error::ErrorEnvelope)),
    security(("session" = [])))]
async fn put_shell(
    c: Caller,
    State(s): State<AppState>,
    Path(agent): Path<Uuid>,
    crate::http::json::Json(body): crate::http::json::Json<PutTerminalShell>,
) -> Result<axum::Json<TerminalShellPreference>, ApiError> {
    Ok(axum::Json(
        service::put_shell_preference(&s, &c, agent, &body.shell).await?,
    ))
}

#[derive(Debug, Deserialize)]
struct TerminalQuery {
    /// Long-poll ≤ `TERMINAL_WAKE_WAIT` before answering (the dashboard's default is to take
    /// the `202 {waking: true}` and retry on ready).
    wait: Option<bool>,
    /// (FIX-138) `bash` or `zsh`; anything else is 422 `invalid_shell`. Absent: the person's
    /// stored choice (`GET /v1/agents/{agent}/terminal/shell`), bash when none.
    shell: Option<String>,
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
    if !s.ops().terminal.enabled {
        return Err(service::terminal_disabled());
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
    // An enumerated value, never a path (FIX-138): the supervisor maps it to a fixed binary.
    // Absent → the person's stored preference for this agent (FIX-218, C-1145), read by the
    // service once the agent is admitted.
    let shell = match q.shell.as_deref() {
        None => None,
        Some(v) => Some(PtyShell::parse(v).ok_or_else(|| {
            ApiError::unprocessable("invalid_shell", "`shell` must be `bash` or `zsh`.")
        })?),
    };
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
    service::upgrade(s, caller, agent, ws, wait, shell).await
}
