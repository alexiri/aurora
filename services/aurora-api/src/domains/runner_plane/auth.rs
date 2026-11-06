//! Runner anchor-token authentication (SEC-24/SEC-25).

use crate::app::AppState;
use aurora_core::ApiError;
use aurora_core::token::{self, TokenPrefix};
use aurora_db::repo::runners::{self, RunnerRow};
use axum::extract::FromRequestParts;
use http::{StatusCode, header, request::Parts};
use uuid::Uuid;

/// Highest protocol version understood; N-1 is accepted too.
const PROTOCOL: u32 = 1;

/// `Authorization: Bearer aur_…` only (used by `register`).
pub struct RunnerToken(pub RunnerRow);

/// Token plus a live replica (`X-Aurora-Replica`); each call is a heartbeat.
pub struct RunnerCtx {
    pub runner: RunnerRow,
    pub replica: Uuid,
}

fn check_protocol(parts: &Parts) -> Result<(), ApiError> {
    if let Some(v) = parts
        .headers
        .get("x-aurora-protocol")
        .and_then(|v| v.to_str().ok())
    {
        let n = v
            .strip_prefix("runner/")
            .and_then(|n| n.parse::<u32>().ok());
        if n.is_none_or(|n| n > PROTOCOL) {
            return Err(ApiError::new(
                StatusCode::UPGRADE_REQUIRED,
                "protocol_unsupported",
                "Unsupported runner protocol version.",
            )
            .with_details(serde_json::json!({"supported": ["runner/0", "runner/1"]})));
        }
    }
    Ok(())
}

impl FromRequestParts<AppState> for RunnerToken {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, ApiError> {
        check_protocol(parts)?;
        let tok = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .filter(|t| token::parse_prefix(t) == Some(TokenPrefix::Runner))
            .ok_or_else(|| ApiError::unauthenticated("Runner token required."))?;
        let row = runners::find_by_token_hash(&st.pool, &token::hash(tok))
            .await?
            .ok_or_else(|| ApiError::unauthenticated("Unknown runner token."))?;
        Ok(Self(row))
    }
}

impl FromRequestParts<AppState> for RunnerCtx {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, ApiError> {
        let RunnerToken(runner) = RunnerToken::from_request_parts(parts, st).await?;
        let replica = parts
            .headers
            .get("x-aurora-replica")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| Uuid::parse_str(v).ok())
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::FORBIDDEN,
                    "replica_unknown",
                    "X-Aurora-Replica missing or invalid; register again.",
                )
            })?;
        if !runners::touch_replica(&st.pool, runner.id, replica).await? {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "replica_unknown",
                "Replica unknown or retired; register again.",
            ));
        }
        Ok(Self { runner, replica })
    }
}
