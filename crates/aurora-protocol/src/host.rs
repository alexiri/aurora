//! Connector-host plane messages (`/v1/connector-host/*`, P4-MCON, C-455..C-457). Normative:
//! `docs/13-channels-connectors.md` §3.1 and `docs/API_CONTRACTS.md`.
//!
//! The host authenticates with its `auh_` anchor token. `assignments` carries decrypted secret
//! settings, so [`Assignment`] never prints them (`Debug` redacts).

use std::collections::BTreeMap;
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::common::{Validate, ValidationError, check_chars, check_max};
use crate::connector::ConnectorManifest;

/// Max types announced by one `register`.
pub const MAX_HOST_TYPES: usize = 32;
/// Max chars of a status message.
pub const MAX_STATUS_MESSAGE_CHARS: usize = 300;
/// Longest `wait` of the assignments long poll, seconds.
pub const MAX_ASSIGNMENTS_WAIT_SECS: u32 = 30;
/// Default `wait` of the assignments long poll, seconds.
pub const DEFAULT_ASSIGNMENTS_WAIT_SECS: u32 = 25;
/// Default heartbeat interval advertised by `register`, seconds.
pub const DEFAULT_HOST_POLL_SECS: u32 = 30;
/// Header naming the managed connector a host acts for on `/v1/connector/*` (C-458).
pub const HDR_CONNECTOR: &str = "aurora-connector";

/// One built-in connector type a host can run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostType {
    pub type_id: String,
    pub manifest: ConnectorManifest,
    /// Detached signature over sha256 of the canonical manifest JSON (as connector `register`).
    pub manifest_signature: String,
}

/// `POST /v1/connector-host/register` (C-455).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostRegisterRequest {
    pub instance_id: Uuid,
    pub version: String,
    pub types: Vec<HostType>,
}

impl Validate for HostRegisterRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_chars("version", &self.version, 64)?;
        check_max("types", self.types.len(), MAX_HOST_TYPES)?;
        for t in &self.types {
            check_chars("types.type_id", &t.type_id, 64)?;
            check_chars("types.manifest_signature", &t.manifest_signature, 4096)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    Signature,
    IdMismatch,
    InvalidManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RejectedType {
    pub type_id: String,
    pub reason: RejectReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostRegisterResponse {
    pub poll_interval_secs: u32,
    pub rejected: Vec<RejectedType>,
}

/// One managed connector the host must run (`GET /v1/connector-host/assignments`, C-456).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Assignment {
    pub id: String,
    pub type_id: String,
    pub display_name: String,
    /// Non-secret settings, defaults applied (incl. `public_url`).
    pub settings: Value,
    /// **Decrypted** secret and generated settings. Never log.
    pub secrets: BTreeMap<String, String>,
    pub status: String,
}

impl fmt::Debug for Assignment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Assignment")
            .field("id", &self.id)
            .field("type_id", &self.type_id)
            .field("display_name", &self.display_name)
            .field("settings", &self.settings)
            .field(
                "secrets",
                &format_args!("[{} redacted]", self.secrets.len()),
            )
            .field("status", &self.status)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AssignmentsResponse {
    pub revision: i64,
    pub connectors: Vec<Assignment>,
}

impl fmt::Debug for AssignmentsResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssignmentsResponse")
            .field("revision", &self.revision)
            .field("connectors", &self.connectors)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InstanceState {
    Running,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConnectorReport {
    pub id: String,
    pub state: InstanceState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// `POST /v1/connector-host/status` (C-457).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostStatusRequest {
    pub instance_id: Uuid,
    pub connectors: Vec<ConnectorReport>,
}

impl Validate for HostStatusRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_max("connectors", self.connectors.len(), 256)?;
        for c in &self.connectors {
            check_chars("connectors.id", &c.id, 64)?;
        }
        Ok(())
    }
}
