//! Relay-plane DTOs (phase 4, P4-PRV-2, D-146, D-147; `16 §W17`). Machine-plane shapes: typed
//! bodies, bounded batches. Not in the dashboard OpenAPI document (the relay is the only client).
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------- register

#[derive(Debug, Deserialize)]
pub struct RelayRegisterRequest {
    pub version: String,
}

#[derive(Debug, Serialize)]
pub struct RelayRegisterResponse {
    pub replica_id: Uuid,
    pub protocol: u32,
    pub config: RelayConfig,
}

/// The relay refuses to serve when its own `previews.domain` differs from this one.
#[derive(Debug, Serialize)]
pub struct RelayConfig {
    pub previews_domain: String,
    /// The dashboard origin (the relay redirects navigations to `<external_url>/v1/previews/authorize`).
    pub external_url: String,
    pub auth_cache_secs: u32,
    pub wake_wait_secs: u32,
}

// ---------------------------------------------------------------- resolve

#[derive(Debug, Deserialize)]
pub struct RelayResolveRequest {
    pub label: String,
    /// `hex(SHA-256 of the cookie token)`; absent = no credential presented.
    #[serde(default)]
    pub session_hash: Option<String>,
    /// Long-poll until `listening` or the timeout, at most `PREVIEW_WAKE_WAIT`.
    #[serde(default)]
    pub wait_secs: Option<u32>,
    /// A top-level navigation (the relay only redirects those to `authorize`).
    #[serde(default)]
    pub navigation: bool,
}

#[derive(Debug, Serialize)]
pub struct RelayResolveResponse {
    /// `allow` | `login` | `deny`.
    pub decision: String,
    /// Uniform `not_found` or the specific `paused`/`frozen`/`wake_limited`/`unreachable`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share: Option<RelayShareRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<RelayRoute>,
}

#[derive(Debug, Serialize)]
pub struct RelayShareRef {
    pub id: Uuid,
    pub port: i32,
}

#[derive(Debug, Serialize)]
pub struct RelayRoute {
    /// `running` | `waking` | `stopped` | `unavailable`.
    pub sandbox: String,
    pub listening: bool,
    /// `none` | `starting` | `exited` | `limited`.
    pub start: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u32>,
}

// ---------------------------------------------------------------- grants/redeem

#[derive(Debug, Deserialize)]
pub struct RelayRedeemRequest {
    pub grant: String,
    pub label: String,
    pub nonce: String,
}

#[derive(Debug, Serialize)]
pub struct RelayRedeemResponse {
    pub token: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub idle_secs: u32,
}

// ---------------------------------------------------------------- streams

#[derive(Debug, Deserialize)]
pub struct RelayStreamsRequest {
    pub label: String,
    pub session_hash: String,
}

#[derive(Debug, Serialize)]
pub struct RelayStreamsResponse {
    pub grant: String,
    pub expires_in: u32,
    pub runner_endpoint: String,
}

// ---------------------------------------------------------------- validate

#[derive(Debug, Deserialize)]
pub struct RelayValidateRequest {
    pub sessions: Vec<RelayValidateEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RelayValidateEntry {
    pub label: String,
    pub session_hash: String,
}

#[derive(Debug, Serialize)]
pub struct RelayValidateResponse {
    /// The entries that no longer pass; the relay closes their streams with 4403.
    pub revoked: Vec<RelayValidateEntry>,
}

// ---------------------------------------------------------------- report

#[derive(Debug, Deserialize)]
pub struct RelayReportRequest {
    pub shares: Vec<RelayReportShare>,
}

#[derive(Debug, Deserialize)]
pub struct RelayReportShare {
    pub share_id: Uuid,
    #[serde(default)]
    pub streams_open: i32,
    #[serde(default)]
    pub streams_total: u64,
    #[serde(default)]
    pub bytes_to_app: u64,
    #[serde(default)]
    pub bytes_from_app: u64,
    #[serde(default)]
    pub last_request_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Serialize)]
pub struct RelayOkResponse {
    pub ok: bool,
}
