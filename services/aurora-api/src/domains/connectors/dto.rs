use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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
    /// Connector id: `[a-z][a-z0-9-]{1,31}`; for a managed connector, a type of
    /// `GET /v1/admin/connector-types`.
    pub id: String,
    /// Name shown to administrators (1-80 chars). Required for a self-hosted connector; a
    /// managed connector defaults to the manifest name.
    #[serde(default)]
    pub display_name: Option<String>,
    /// `true`: add a managed connector run by a connector host (C-451). Default `false`.
    #[serde(default)]
    pub managed: bool,
    /// Managed: non-secret `instance_setup` values.
    #[serde(default)]
    #[schema(value_type = Option<Object>)]
    pub settings: Option<serde_json::Map<String, serde_json::Value>>,
    /// Managed: values of the `secret` fields (write-only, at most 8 KiB each).
    #[serde(default)]
    pub secrets: Option<BTreeMap<String, String>>,
    /// Managed (required): the manifest digest the administrator reviewed.
    #[serde(default)]
    pub approve_manifest_digest: Option<String>,
}

/// `PATCH /v1/admin/connectors/{id}` (C-453).
#[derive(Deserialize, ToSchema)]
pub struct PatchConnectorRequest {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Managed only: merged per key; `null` restores the field default.
    #[serde(default)]
    #[schema(value_type = Option<Object>)]
    pub settings: Option<serde_json::Map<String, serde_json::Value>>,
    /// Managed only: a string replaces the secret, `null` clears it (not for required ones).
    #[serde(default)]
    pub secrets: Option<BTreeMap<String, Option<String>>>,
    /// Managed only: `generated_secret` fields to generate again.
    #[serde(default)]
    pub regenerate: Option<Vec<String>>,
}

/// State of one secret setting; the value is never returned.
#[derive(Serialize, ToSchema)]
pub struct ConnectorSecretInfo {
    pub set: bool,
    pub generated: bool,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HostState {
    Running,
    Error,
}

/// What one connector host reports for a managed connector.
#[derive(Serialize, ToSchema)]
pub struct HostStatusInfo {
    pub host_instance: Uuid,
    pub state: HostState,
    pub message: Option<String>,
    pub at: DateTime<Utc>,
}

/// A connector type announced by live connector hosts (C-450).
#[derive(Serialize, ToSchema)]
pub struct ConnectorType {
    pub type_id: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub icon_png_b64: Option<String>,
    pub setup_guide_md: Option<String>,
    /// The manifest, icon stripped.
    #[schema(value_type = Object)]
    pub manifest: serde_json::Value,
    pub manifest_digest: String,
    /// `{ fields: Field[] }`: the form-field schema plus `url`, `number`, `generated_secret`.
    #[schema(value_type = Object)]
    pub instance_setup: serde_json::Value,
    /// Live hosts announcing exactly this digest.
    pub hosts: i64,
    /// `false` for a type accepted without a signature under `plugins.allow_unsigned`
    /// (development only, D-142).
    pub signed: bool,
    /// A connector with this id already exists.
    pub added: bool,
}

#[derive(Serialize, ToSchema)]
pub struct ConnectorTypeList {
    /// At least one connector host was seen within `CONNECTOR_HOST_STALE`.
    pub host_running: bool,
    pub items: Vec<ConnectorType>,
}

#[derive(Serialize, ToSchema)]
pub struct ConnectorHostInfo {
    pub instance_id: Uuid,
    pub version: String,
    pub last_seen_at: DateTime<Utc>,
    pub types: Vec<String>,
}

/// `GET /v1/admin/overview` (C-459): this phase reports the connector hosts.
#[derive(Serialize, ToSchema)]
pub struct AdminOverview {
    /// Connector hosts seen within 24 hours.
    pub connector_hosts: Vec<ConnectorHostInfo>,
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
    /// The last approved manifest (icon stripped), the baseline for the approval diff (C-40).
    #[schema(value_type = Option<Object>)]
    pub approved_manifest: Option<serde_json::Value>,
    /// Human-readable reasons approval is required (empty unless `pending_approval`).
    pub widening: Vec<String>,
    /// Live (not retired, recently seen) replicas.
    pub replicas: Vec<ReplicaInfo>,
    pub integration_count: i64,
    pub approved_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    /// Added from the dashboard and run by a connector host (13 §3.1).
    pub managed: bool,
    /// Managed only: the built-in type.
    pub type_id: Option<String>,
    /// Managed only: stored non-secret settings with defaults applied; `null` otherwise.
    #[schema(value_type = Option<Object>)]
    pub settings: Option<serde_json::Value>,
    /// Managed only: one entry per `secret` / `generated_secret` field; `{}` otherwise.
    pub secrets: BTreeMap<String, ConnectorSecretInfo>,
    /// Managed only: reports of hosts seen recently.
    pub host_status: Vec<HostStatusInfo>,
}

#[derive(Serialize, ToSchema)]
pub struct ConnectorAdminList {
    pub items: Vec<ConnectorAdmin>,
}

/// Returned on provisioning (C-31, C-451): the `aun_` token is shown once and only for a
/// self-hosted connector.
#[derive(Serialize, ToSchema)]
pub struct ConnectorWithToken {
    pub connector: ConnectorAdmin,
    /// `aun_…`, shown once; absent for a managed connector.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
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
