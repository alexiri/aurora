use aurora_db::repo::ssh::SshKeyRow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// A person's registered SSH public key (C-790).
#[derive(Debug, Serialize, ToSchema)]
pub struct SshKey {
    pub id: Uuid,
    pub name: String,
    /// Always `ssh-ed25519`.
    pub algorithm: String,
    /// Canonical OpenSSH form `ssh-ed25519 <base64>` (no comment).
    pub public_key: String,
    /// `SHA256:` + unpadded base64 of the key blob; equals `ssh-keygen -lf`.
    pub fingerprint: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

impl From<SshKeyRow> for SshKey {
    fn from(r: SshKeyRow) -> Self {
        Self {
            id: r.id,
            name: r.name,
            algorithm: r.algorithm,
            public_key: r.public_key,
            fingerprint: r.fingerprint,
            created_at: r.created_at,
            last_used_at: r.last_used_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SshKeyList {
    /// Newest first.
    pub keys: Vec<SshKey>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateSshKeyRequest {
    /// One `ssh-ed25519 <base64> [comment]` line, at most 4096 bytes.
    pub public_key: String,
    /// At most 80 characters; default: the key's comment, else "SSH key".
    pub name: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct RenameSshKeyRequest {
    pub name: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct MintCertificateRequest {
    /// The registered key the certificate certifies (its fingerprint must be one of the
    /// caller's `ssh_keys`).
    pub public_key: String,
}

/// A minted SSH user certificate (C-791).
#[derive(Debug, Serialize, ToSchema)]
pub struct SshCertificate {
    /// The `ssh-ed25519-cert-v01@openssh.com` line, ready for `id_ed25519-cert.pub`.
    pub certificate: String,
    /// The serial as a decimal string (a u64).
    pub serial: String,
    /// The agent id (the certificate's only principal).
    pub principal: String,
    pub key_id: String,
    pub valid_after: DateTime<Utc>,
    pub valid_before: DateTime<Utc>,
    /// The SSH user name to connect as: the agent id.
    pub username: String,
    pub host: String,
    pub port: u16,
    /// `known_hosts` lines for the terminators' host keys (empty until one registers).
    pub known_hosts: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HostKey {
    pub public_key: String,
    pub fingerprint: String,
}

/// Connection facts (C-792).
#[derive(Debug, Serialize, ToSchema)]
pub struct SshFacts {
    pub host: String,
    pub port: u16,
    pub cert_ttl_secs: u64,
    /// SSH terminator replicas seen in the last 90 s (`0` until one registers).
    pub terminators_live: u32,
    pub host_keys: Vec<HostKey>,
    pub known_hosts: Vec<String>,
}

/// The agent an SSH session is on (C-793).
#[derive(Debug, Serialize, ToSchema)]
pub struct SshSessionAgent {
    pub id: Uuid,
    pub name: String,
}

/// The person behind a session, shown to `can_own` only.
#[derive(Debug, Serialize, ToSchema)]
pub struct SshSessionUser {
    pub id: Uuid,
    pub label: String,
}

/// The key a session authenticated with; `id`/`name` are null once the key is deleted.
#[derive(Debug, Serialize, ToSchema)]
pub struct SshSessionKey {
    pub id: Option<Uuid>,
    pub name: Option<String>,
    pub fingerprint: String,
}

/// An open SSH session (C-793): names, a coarse client address and counts; never content.
#[derive(Debug, Serialize, ToSchema)]
pub struct SshSession {
    pub id: Uuid,
    pub agent: SshSessionAgent,
    pub workspace_id: Uuid,
    /// Present for `can_own` listings of an agent's sessions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<SshSessionUser>,
    pub key: SshSessionKey,
    /// Coarsened at insert (IPv4 /24, IPv6 /48).
    pub client_address: Option<String>,
    pub opened_at: DateTime<Utc>,
    pub last_activity_at: Option<DateTime<Utc>>,
    /// Channels open at the last heartbeat is not tracked by the API; `0` until then.
    pub channels_open: u32,
    pub channels_total: u32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SshSessionList {
    pub sessions: Vec<SshSession>,
}

/// 202 body of `DELETE /v1/ssh-sessions/{id}`.
#[derive(Debug, Serialize, ToSchema)]
pub struct Disconnecting {
    pub disconnecting: bool,
}

/// One registered terminator replica (`GET /v1/admin/terminators`, C-793).
#[derive(Debug, Serialize, ToSchema)]
pub struct TerminatorInfo {
    pub id: Uuid,
    pub version: String,
    pub host_key_fingerprint: String,
    pub sessions_open: u32,
    pub last_seen_at: DateTime<Utc>,
    pub live: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TerminatorList {
    pub terminators: Vec<TerminatorInfo>,
}
