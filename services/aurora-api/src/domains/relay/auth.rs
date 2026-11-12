//! Relay anchor-token authentication (phase 4, P4-PRV-2, D-146; `16 §W17`).
//!
//! Every relay-plane request carries `Authorization: Bearer aul_…` and, after registration,
//! `X-Aurora-Replica: <replica_id>` and `X-Aurora-Protocol: relay/1`. An unknown, expired or
//! malformed `aul_` is 401 with the same body for every cause (SEC-85: no oracle).

use crate::app::AppState;
use aurora_core::{
    ApiError,
    token::{self, TokenPrefix},
};
use aurora_db::repo::preview_relay as repo;
use axum::extract::FromRequestParts;
use http::{StatusCode, header, request::Parts};
use uuid::Uuid;

/// Highest relay protocol version understood.
const PROTOCOL: u32 = 1;

fn unauthenticated() -> ApiError {
    // One identical body for every 401 cause (`16 §W17`).
    ApiError::unauthenticated("Relay token required.")
}

fn check_protocol(parts: &Parts) -> Result<(), ApiError> {
    if let Some(v) = parts
        .headers
        .get("x-aurora-protocol")
        .and_then(|v| v.to_str().ok())
    {
        let n = v.strip_prefix("relay/").and_then(|n| n.parse::<u32>().ok());
        if n.is_none_or(|n| n > PROTOCOL) {
            return Err(ApiError::new(
                StatusCode::UPGRADE_REQUIRED,
                "protocol_unsupported",
                "Unsupported relay protocol version.",
            )
            .with_details(serde_json::json!({"supported": ["relay/1"]})));
        }
    }
    Ok(())
}

/// `Authorization: Bearer aul_…` only (used by `register`).
pub struct RelayToken(pub Uuid);

impl FromRequestParts<AppState> for RelayToken {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, ApiError> {
        check_protocol(parts)?;
        let tok = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .filter(|t| token::parse_prefix(t) == Some(TokenPrefix::Relay))
            .ok_or_else(unauthenticated)?;
        let id = repo::find_relay_token(&st.pool, &token::hash(tok))
            .await?
            .ok_or_else(unauthenticated)?;
        Ok(Self(id))
    }
}

/// Token plus a registered replica (`X-Aurora-Replica`).
pub struct RelayCtx {
    pub token_id: Uuid,
    pub replica: Uuid,
}

impl FromRequestParts<AppState> for RelayCtx {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, ApiError> {
        let RelayToken(token_id) = RelayToken::from_request_parts(parts, st).await?;
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
        if !repo::relay_replica_owned(&st.pool, replica, token_id).await? {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "replica_unknown",
                "Replica unknown or pruned; register again.",
            ));
        }
        Ok(Self { token_id, replica })
    }
}
