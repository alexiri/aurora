//! Connector anchor-token authentication for `/v1/connector/*` (docs/13 §4, §6.5).

use crate::app::AppState;
use aurora_core::ApiError;
use aurora_core::token::{self, TokenPrefix};
use aurora_db::repo::connector_hosts;
use aurora_db::repo::connectors::{self, ConnectorRow};
use aurora_protocol::connector::HDR_REPLICA;
use aurora_protocol::host::HDR_CONNECTOR;
use axum::extract::FromRequestParts;
use http::{StatusCode, header, request::Parts};
use uuid::Uuid;

/// Highest `connector/N` contract understood.
const PROTOCOL: u32 = 1;

/// `Authorization: Bearer aun_…` only (used by `register`).
pub struct ConnectorToken(pub ConnectorRow);

/// Token plus a live replica (`X-Aurora-Replica`); each call is a heartbeat.
pub struct ConnectorCtx {
    pub connector: ConnectorRow,
    pub replica: Uuid,
}

fn check_protocol(parts: &Parts) -> Result<(), ApiError> {
    if let Some(v) = parts
        .headers
        .get(aurora_protocol::version::HEADER)
        .and_then(|v| v.to_str().ok())
    {
        let n = v
            .strip_prefix("connector/")
            .and_then(|n| n.parse::<u32>().ok());
        if n.is_none_or(|n| n > PROTOCOL) {
            return Err(ApiError::new(
                StatusCode::UPGRADE_REQUIRED,
                "protocol_unsupported",
                "Unsupported connector protocol version.",
            )
            .with_details(serde_json::json!({"supported": ["connector/1"]})));
        }
    }
    Ok(())
}

/// C-458: a connector host's `auh_` token plus `Aurora-Connector: <id>` acts as connector `<id>`,
/// only when that connector is `managed` and not `disabled`. Every refusal is the same 401 as an
/// unknown `aun_` token, so nothing reveals which connectors exist.
async fn delegated(parts: &Parts, st: &AppState, tok: &str) -> Result<ConnectorRow, ApiError> {
    let refuse = || ApiError::unauthenticated("Unknown connector token.");
    let id = parts
        .headers
        .get(HDR_CONNECTOR)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(refuse)?;
    if !connector_hosts::token_exists(&st.pool, &token::hash(tok)).await? {
        return Err(refuse());
    }
    connectors::get(&st.pool, id)
        .await?
        .filter(|r| r.managed && r.status != "disabled")
        .ok_or_else(refuse)
}

impl FromRequestParts<AppState> for ConnectorToken {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, ApiError> {
        check_protocol(parts)?;
        let tok = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(|| ApiError::unauthenticated("Connector token required."))?;
        if token::parse_prefix(tok) == Some(TokenPrefix::ConnectorHost) {
            return delegated(parts, st, tok).await.map(Self);
        }
        if token::parse_prefix(tok) != Some(TokenPrefix::Connector) {
            return Err(ApiError::unauthenticated("Connector token required."));
        }
        // With an `aun_` token the `Aurora-Connector` header is ignored (C-458).
        let row = connectors::find_by_token_hash(&st.pool, &token::hash(tok))
            .await?
            .ok_or_else(|| ApiError::unauthenticated("Unknown connector token."))?;
        if row.status == "disabled" {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "connector_disabled",
                "This connector is disabled.",
            ));
        }
        Ok(Self(row))
    }
}

impl FromRequestParts<AppState> for ConnectorCtx {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, ApiError> {
        let ConnectorToken(connector) = ConnectorToken::from_request_parts(parts, st).await?;
        let replica = parts
            .headers
            .get(HDR_REPLICA)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| Uuid::parse_str(v).ok())
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::FORBIDDEN,
                    "replica_unknown",
                    "X-Aurora-Replica missing or invalid; register again.",
                )
            })?;
        if !connectors::touch_replica(&st.pool, &connector.id, replica).await? {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "replica_unknown",
                "Replica unknown or retired; register again.",
            ));
        }
        Ok(Self { connector, replica })
    }
}
