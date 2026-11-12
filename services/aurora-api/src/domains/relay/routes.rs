//! Relay-plane routes (phase 4, P4-PRV-2, D-146; `16 §W17`): `/v1/relay/*` (the only client
//! is the relay; bearer `aul_`) plus the runner-plane preview-stream endpoints and the
//! dashboard-side `GET /v1/previews/authorize`. Mounted under `/v1`; plain routes, excluded
//! from the dashboard OpenAPI document (the relay plane has its own client).
use super::auth::{RelayCtx, RelayToken};
use super::dto::*;
use super::service;
use crate::app::AppState;
use crate::http::auth::Caller;
use crate::http::json::Json;
use aurora_core::ApiError;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use utoipa_axum::router::OpenApiRouter;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .route("/relay/register", axum::routing::post(register))
        .route("/relay/resolve", axum::routing::post(resolve))
        .route("/relay/grants/redeem", axum::routing::post(redeem))
        .route("/relay/streams", axum::routing::post(streams))
        .route("/relay/validate", axum::routing::post(validate))
        .route("/relay/report", axum::routing::post(report))
        .route(
            "/runner/preview-streams/open",
            axum::routing::post(preview_stream_open),
        )
        .route(
            "/runner/preview-streams/closed",
            axum::routing::post(preview_stream_closed),
        )
        .route("/previews/authorize", axum::routing::get(authorize))
}

async fn register(
    t: RelayToken,
    State(s): State<AppState>,
    Json(req): Json<RelayRegisterRequest>,
) -> Result<Json<RelayRegisterResponse>, ApiError> {
    Ok(Json(service::register(&s, t.0, req).await?))
}

async fn resolve(
    _c: RelayCtx,
    State(s): State<AppState>,
    Json(req): Json<RelayResolveRequest>,
) -> Result<Json<RelayResolveResponse>, ApiError> {
    Ok(Json(service::resolve(&s, req).await?))
}

async fn redeem(
    _c: RelayCtx,
    State(s): State<AppState>,
    Json(req): Json<RelayRedeemRequest>,
) -> Result<Json<RelayRedeemResponse>, ApiError> {
    Ok(Json(service::redeem(&s, req).await?))
}

async fn streams(
    _c: RelayCtx,
    State(s): State<AppState>,
    Json(req): Json<RelayStreamsRequest>,
) -> Result<Json<RelayStreamsResponse>, ApiError> {
    Ok(Json(service::streams(&s, req).await?))
}

async fn validate(
    _c: RelayCtx,
    State(s): State<AppState>,
    Json(req): Json<RelayValidateRequest>,
) -> Result<Json<RelayValidateResponse>, ApiError> {
    Ok(Json(service::validate(&s, req).await?))
}

async fn report(
    c: RelayCtx,
    State(s): State<AppState>,
    Json(req): Json<RelayReportRequest>,
) -> Result<Json<RelayOkResponse>, ApiError> {
    Ok(Json(service::report(&s, &c, req).await?))
}

// ---------------------------------------------------------------- runner plane additions

async fn preview_stream_open(
    c: crate::domains::runner_plane::RunnerCtx,
    State(s): State<AppState>,
    Json(req): Json<aurora_protocol::runner::PreviewStreamOpenRequest>,
) -> Result<Json<aurora_protocol::runner::PreviewStreamOpenResponse>, ApiError> {
    Ok(Json(
        crate::domains::runner_plane::preview_streams::open(&s, &c.runner, req).await?,
    ))
}

async fn preview_stream_closed(
    c: crate::domains::runner_plane::RunnerCtx,
    State(s): State<AppState>,
    Json(req): Json<aurora_protocol::runner::PreviewStreamClosedRequest>,
) -> Result<Json<RelayOkResponse>, ApiError> {
    crate::domains::runner_plane::preview_streams::closed(&s, &c.runner, req).await?;
    Ok(Json(RelayOkResponse { ok: true }))
}

// ---------------------------------------------------------------- dashboard authorize

#[derive(Debug, serde::Deserialize)]
pub struct AuthorizeQuery {
    pub host: Option<String>,
    pub state: Option<String>,
}

/// `GET /v1/previews/authorize` (docs/04 §Preview access step 2): session-only route on the
/// dashboard origin; 302 to the share's `/_aurora/grant?g=…`, or a redirect to sign-in with a
/// same-origin `return_to` when there is no session.
async fn authorize(
    State(s): State<AppState>,
    Query(q): Query<AuthorizeQuery>,
    mut parts: http::request::Parts,
) -> Response {
    // Session only: without one, redirect to sign-in with a same-origin `return_to` of
    // itself (the flow restarts after the login).
    let caller = match <Caller as axum::extract::FromRequestParts<AppState>>::from_request_parts(
        &mut parts, &s,
    )
    .await
    {
        Ok(c) if c.is_session() => c,
        _ => {
            let path = parts
                .uri
                .path_and_query()
                .map(|pq| pq.as_str().to_owned())
                .unwrap_or_else(|| "/v1/previews/authorize".into());
            let safe = crate::domains::oidc::service::validate_return_to(Some(&path))
                .unwrap_or_else(|_| "/".into());
            // `return_to` is a same-origin path; the only query character it can carry is
            // `&`/`?`, encoded minimally by hand (no extra dependency).
            let enc = safe
                .replace('%', "%25")
                .replace('?', "%3F")
                .replace('&', "%26")
                .replace('#', "%23");
            let to = format!("/auth/login?return_to={enc}");
            return (StatusCode::FOUND, [("location", to)]).into_response();
        }
    };
    match service::authorize(&s, &caller, q.host, q.state).await {
        Ok((location, _cookie)) => (StatusCode::FOUND, [("location", location)]).into_response(),
        // Every failure of this route answers the same 404 page (the existence of a share is
        // never revealed).
        Err(e) => e.into_response(),
    }
}
