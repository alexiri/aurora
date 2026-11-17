//! The terminator plane `/v1/terminator/*` (phase 5, P4-SSH-2, D-239, C-794; `16 §W20`).
//!
//! One anchor token, `AURORA_BOOTSTRAP_TERMINATOR_TOKEN` (`aut_…`): its SHA-256 is compared in
//! constant time against the presented bearer's, so the value itself never sits in a lookup.
//! Every refusal at the door is the same 401 (no oracle). After `register` a request names its
//! replica (`X-Aurora-Replica`) and is charged to that replica's own rate class
//! (`SSH_API_RATE`/s, burst twice that). Plain routes, excluded from the dashboard OpenAPI
//! document. Off while `ssh.enabled` is off (404 `ssh_disabled`).
use super::sessions::{self, OpenError};
use crate::app::AppState;
use crate::http::json::{Json, Path};
use crate::http::ratelimit::Quota;
use aurora_core::ApiError;
use aurora_core::limits::SSH_API_RATE;
use aurora_core::token::{self, TokenPrefix};
use aurora_db::repo::ssh_sessions as repo;
use aurora_protocol::terminator as wire;
use axum::extract::{FromRequestParts, State};
use axum::http::StatusCode;
use http::{header, request::Parts};
use std::time::Duration;
use utoipa_axum::router::OpenApiRouter;
use uuid::Uuid;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .route("/terminator/register", axum::routing::post(register))
        .route("/terminator/sessions", axum::routing::post(open))
        .route(
            "/terminator/sessions/{id}/channels",
            axum::routing::post(channels),
        )
        .route(
            "/terminator/sessions/{id}/close",
            axum::routing::post(close),
        )
        .route("/terminator/heartbeat", axum::routing::post(heartbeat))
}

fn unauthenticated() -> ApiError {
    // One identical body for every 401 cause.
    ApiError::unauthenticated("Terminator token required.")
}

/// The anchor token alone (used by `register`).
pub struct TerminatorToken;

impl FromRequestParts<AppState> for TerminatorToken {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, s: &AppState) -> Result<Self, ApiError> {
        if !s.config.ssh.enabled {
            return Err(super::service::ssh_disabled());
        }
        let presented = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .filter(|t| token::parse_prefix(t) == Some(TokenPrefix::SshTerminator));
        let (Some(presented), Some(anchor)) =
            (presented, s.config.bootstrap_terminator_token.as_ref())
        else {
            return Err(unauthenticated());
        };
        // Both sides hashed first: the comparison is over equal-length digests and never over
        // the secret itself.
        let ok = aws_lc_rs::constant_time::verify_slices_are_equal(
            &token::hash(presented),
            &token::hash(anchor.expose()),
        )
        .is_ok();
        if ok { Ok(Self) } else { Err(unauthenticated()) }
    }
}

/// The anchor token plus a registered replica (`X-Aurora-Replica`), charged to its rate class.
pub struct TerminatorCtx {
    pub replica: Uuid,
}

impl FromRequestParts<AppState> for TerminatorCtx {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, s: &AppState) -> Result<Self, ApiError> {
        TerminatorToken::from_request_parts(parts, s).await?;
        let replica = parts
            .headers
            .get("x-aurora-replica")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| Uuid::parse_str(v).ok())
            .ok_or_else(replica_unknown)?;
        s.rate_limiter.check_quota(
            "ssh_plane",
            &replica.to_string(),
            Quota {
                capacity: SSH_API_RATE * 2,
                per: Duration::from_secs(2),
            },
        )?;
        if !repo::replica_exists(&s.pool, replica).await? {
            return Err(replica_unknown());
        }
        Ok(Self { replica })
    }
}

/// The wire bounds of `aurora-protocol` (sizes, `wait_secs`): 400 on a violation.
fn check<T: aurora_protocol::common::Validate>(t: &T) -> Result<(), ApiError> {
    t.validate().map_err(|e| {
        ApiError::invalid_request(
            "Invalid request.",
            vec![aurora_core::FieldError::new("/", e.to_string())],
        )
    })
}

fn replica_unknown() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "replica_unknown",
        "X-Aurora-Replica missing, invalid or pruned; register again.",
    )
}

async fn register(
    _t: TerminatorToken,
    State(s): State<AppState>,
    Json(req): Json<wire::RegisterRequest>,
) -> Result<Json<wire::RegisterResponse>, ApiError> {
    check(&req)?;
    Ok(Json(sessions::register(&s, req).await?))
}

async fn open(
    c: TerminatorCtx,
    State(s): State<AppState>,
    Json(req): Json<wire::SessionOpenRequest>,
) -> Result<(StatusCode, Json<wire::SessionOpenResponse>), OpenError> {
    check(&req)?;
    Ok((
        StatusCode::CREATED,
        Json(sessions::open_session(&s, c.replica, req).await?),
    ))
}

async fn channels(
    c: TerminatorCtx,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<wire::ChannelGrantRequest>,
) -> Result<Json<wire::ChannelGrantResponse>, ApiError> {
    check(&req)?;
    Ok(Json(sessions::channel_grant(&s, c.replica, id, req).await?))
}

async fn close(
    c: TerminatorCtx,
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<wire::SessionCloseRequest>,
) -> Result<Json<wire::OkBody>, ApiError> {
    check(&req)?;
    Ok(Json(sessions::close_session(&s, c.replica, id, req).await?))
}

async fn heartbeat(
    c: TerminatorCtx,
    State(s): State<AppState>,
    Json(req): Json<wire::HeartbeatRequest>,
) -> Result<Json<wire::HeartbeatResponse>, ApiError> {
    check(&req)?;
    Ok(Json(sessions::heartbeat(&s, c.replica, req).await?))
}
