use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Deserialize, ToSchema)]
pub struct CreateRunnerRequest {
    /// Unique runner name (1-64 chars).
    pub name: String,
}

#[derive(Serialize, ToSchema)]
pub struct Replica {
    pub id: Uuid,
    pub advertised_host: String,
    pub version: String,
    pub started_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub retired_at: Option<DateTime<Utc>>,
    /// Seen within the online threshold and not retired.
    pub live: bool,
}

#[derive(Serialize, ToSchema)]
pub struct RunnerSummary {
    pub id: Uuid,
    pub name: String,
    /// First 16 hex of `sha256(token)`; labels every infrastructure object.
    pub installation_fingerprint: String,
    pub backend: Option<String>,
    /// Declared capabilities (opaque JSON), `null` until the first registration.
    #[schema(value_type = Option<Object>)]
    pub capabilities: Option<serde_json::Value>,
    pub last_registered_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub live_replicas: i64,
    pub sandboxes: i64,
    pub running_sandboxes: i64,
    pub max_sandboxes: Option<i64>,
    pub replicas: Vec<Replica>,
}

#[derive(Serialize, ToSchema)]
pub struct RunnerList {
    pub items: Vec<RunnerSummary>,
}

/// Returned once on provisioning and rotation.
#[derive(Serialize, ToSchema)]
pub struct RunnerWithToken {
    pub runner: RunnerSummary,
    /// `aur_…`, shown once.
    pub token: String,
}
