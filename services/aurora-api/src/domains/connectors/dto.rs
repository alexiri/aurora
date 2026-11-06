use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorStatus {
    AwaitingRegistration,
    PendingApproval,
    Active,
    Disabled,
}

impl ConnectorStatus {
    pub fn parse(s: &str) -> Self {
        match s {
            "pending_approval" => Self::PendingApproval,
            "active" => Self::Active,
            "disabled" => Self::Disabled,
            _ => Self::AwaitingRegistration,
        }
    }
}

#[derive(Deserialize, ToSchema)]
pub struct CreateConnectorRequest {
    /// Connector id: `[a-z][a-z0-9-]{1,31}`.
    pub id: String,
    /// Name shown to administrators (1-80 chars).
    pub display_name: String,
}

#[derive(Deserialize, ToSchema)]
pub struct ApproveConnectorRequest {
    /// Digest of exactly the manifest being approved (`manifest_digest` of the connector).
    pub manifest_digest: String,
}

#[derive(Serialize, ToSchema)]
pub struct ReplicaInfo {
    pub id: Uuid,
    pub version: String,
    pub last_seen_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
pub struct ConnectorAdmin {
    pub id: String,
    pub display_name: String,
    pub status: ConnectorStatus,
    pub version: Option<String>,
    pub contract: Option<u32>,
    /// The announced manifest (opaque JSON), `null` until the first registration.
    #[schema(value_type = Option<Object>)]
    pub manifest: Option<serde_json::Value>,
    pub manifest_digest: Option<String>,
    pub approved_manifest_digest: Option<String>,
    /// Human-readable reasons approval is required (empty unless `pending_approval`).
    pub widening: Vec<String>,
    /// Live (not retired, recently seen) replicas.
    pub replicas: Vec<ReplicaInfo>,
    pub integration_count: i64,
    pub approved_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
pub struct ConnectorAdminList {
    pub items: Vec<ConnectorAdmin>,
}

/// Returned once on provisioning.
#[derive(Serialize, ToSchema)]
pub struct ConnectorWithToken {
    pub connector: ConnectorAdmin,
    /// `aun_…`, shown once.
    pub token: String,
}

/// Returned once on rotation.
#[derive(Serialize, ToSchema)]
pub struct ConnectorToken {
    /// `aun_…`, shown once.
    pub token: String,
}

#[derive(Serialize, ToSchema)]
pub struct ConnectorInfo {
    pub id: String,
    pub display_name: String,
    pub icon_url: String,
    /// Raw markdown; render client-side with the safe markdown renderer.
    pub setup_guide_md: Option<String>,
    #[schema(value_type = Object)]
    pub capabilities: serde_json::Value,
    /// `{ fields: FormField[] }`.
    #[schema(value_type = Object)]
    pub integration_setup: serde_json::Value,
    /// `{ dedicated?: { fields: FormField[] } }`.
    #[schema(value_type = Object)]
    pub presence_setup: serde_json::Value,
}

#[derive(Serialize, ToSchema)]
pub struct ConnectorInfoList {
    pub items: Vec<ConnectorInfo>,
}
