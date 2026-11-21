//! The SSH terminator plane (`terminator/1`; phase 5, P4-SSH-2, D-239, C-794). Normative:
//! `docs/16-wire-protocols.md` §W20 and `docs/05` §Terminator plane. The terminator holds no
//! signing key, no database and no cluster credential: it asks the API for every decision.
//! Nothing here carries channel bytes, command lines or the certificate beyond the one
//! `sessions` call that presents it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::common::{SecretString, Validate, ValidationError, check_chars, check_max};

/// The terminator protocol version `register` answers.
pub const TERMINATOR_PROTOCOL: u32 = 1;
/// Max sessions in one heartbeat.
pub const MAX_HEARTBEAT_SESSIONS: usize = 500;
/// Max certificate line bytes accepted by `sessions` (an Ed25519 user certificate is < 1 KiB).
pub const MAX_CERTIFICATE_BYTES: usize = 8 * 1024;

/// `POST /v1/terminator/register`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RegisterRequest {
    pub version: String,
    /// The replica's SSH host public key, `ssh-ed25519 <base64>`.
    pub host_public_key: String,
}

impl Validate for RegisterRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_chars("version", &self.version, 64)?;
        check_chars("host_public_key", &self.host_public_key, 256)
    }
}

/// What the API tells the terminator about the instance (`register` and every heartbeat).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TerminatorConfig {
    /// OpenSSH public-key lines of the CA(s) the terminator may pre-verify certificates with.
    pub ca_public_keys: Vec<String>,
    pub heartbeat_secs: u32,
    pub lease_secs: u32,
    pub wake_wait_secs: u32,
    pub idle_timeout_secs: u32,
    pub max_lifetime_secs: u32,
    pub channels_per_session: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RegisterResponse {
    pub replica_id: Uuid,
    pub protocol: u32,
    pub config: TerminatorConfig,
}

/// `POST /v1/terminator/sessions`: the client's signature was verified by the terminator; the
/// API re-verifies the certificate and decides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionOpenRequest {
    /// The OpenSSH certificate line the client authenticated with.
    pub certificate: SecretString,
    /// The agent id the certificate law returned (the first principal, the key id's agent;
    /// SEC-147), never the user name the client typed.
    pub username: String,
    /// The client's address as the terminator saw it (coarsened by the API at insert).
    #[serde(default)]
    pub client_address: Option<String>,
}

impl Validate for SessionOpenRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_max(
            "certificate",
            self.certificate.expose().len(),
            MAX_CERTIFICATE_BYTES,
        )?;
        check_chars("username", &self.username, 128)?;
        if let Some(a) = &self.client_address {
            check_chars("client_address", a, 64)?;
        }
        Ok(())
    }
}

/// The sandbox's state at session open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SandboxHint {
    Running,
    Waking,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionOpenResponse {
    pub session_id: Uuid,
    pub agent_id: Uuid,
    pub sandbox: SandboxHint,
    pub lease_secs: u32,
    /// RFC 3339 UTC: when the lease runs out unless a heartbeat renews it.
    pub expires_at: String,
}

/// 403 body of `sessions`: the terminator answers every reason with the same hint-free failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionRefusal {
    Denied,
    Limit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionRefusedBody {
    pub reason: SessionRefusal,
}

/// The channel's mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChannelMode {
    Shell,
    Exec,
    Sftp,
}

impl ChannelMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Exec => "exec",
            Self::Sftp => "sftp",
        }
    }
}

/// `POST /v1/terminator/sessions/{id}/channels`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChannelGrantRequest {
    pub mode: ChannelMode,
    /// How long the API may hold the call while a stopped sandbox wakes (≤ 45).
    #[serde(default)]
    pub wait_secs: Option<u32>,
}

impl Validate for ChannelGrantRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        match self.wait_secs {
            Some(w) if w > aurora_core::limits::SSH_CHANNEL_POLL_MAX.as_secs() as u32 => {
                Err(ValidationError::Invalid {
                    field: "wait_secs",
                    reason: "must be at most 45",
                })
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChannelGrantResponse {
    /// The single-use `auz_` grant; returned once.
    pub grant: SecretString,
    pub expires_in: u32,
    /// The runner's `:4108` leg-1 endpoint (`ws://host:4108/v1/preview-stream`).
    pub runner_endpoint: String,
}

/// 409 body of `channels` while the sandbox is not running yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NotRunningDetails {
    pub retry_after_secs: u32,
}

/// One session's heartbeat figures (counts and bytes only; never content).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionBeat {
    pub id: Uuid,
    #[serde(default)]
    /// RFC 3339 UTC of the last channel byte the terminator saw.
    pub last_activity_at: Option<String>,
    pub channels_open: u32,
    pub channels_total: u32,
    pub bytes_in: u64,
    pub bytes_out: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HeartbeatRequest {
    pub sessions: Vec<SessionBeat>,
}

impl Validate for HeartbeatRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_max("sessions", self.sessions.len(), MAX_HEARTBEAT_SESSIONS)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Revoked {
    pub id: Uuid,
    /// `access_revoked`, `key_deleted`, `frozen`, `ssh_disabled`, `user_disconnect`,
    /// `owner_disconnect`, `agent_deleted`, `lease_expired` or `unknown`.
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HeartbeatResponse {
    pub revoked: Vec<Revoked>,
    pub config: TerminatorConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct ChannelsByMode {
    #[serde(default)]
    pub shell: u32,
    #[serde(default)]
    pub exec: u32,
    #[serde(default)]
    pub sftp: u32,
}

/// `POST /v1/terminator/sessions/{id}/close` (idempotent: the first close wins).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionCloseRequest {
    pub reason: String,
    pub channels_total: u32,
    #[serde(default)]
    pub channels_by_mode: ChannelsByMode,
    pub bytes_in: u64,
    pub bytes_out: u64,
}

impl Validate for SessionCloseRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_chars("reason", &self.reason, 64)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OkBody {
    pub ok: bool,
}
