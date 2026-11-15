use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

/// (FIX-121, C-695) Rotation options.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RotateQuery {
    /// Seconds the replaced token keeps working so the runner can be rolled (0 to 86400,
    /// default 600; 0 refuses the old token at once).
    pub grace_secs: Option<u32>,
}

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
    /// The declared capacity limit (`capabilities.max_sandboxes`); `null` = none declared.
    pub max_sandboxes: Option<i64>,
    /// (FIX-121, C-695) While a rotation's grace lasts: when the replaced token stops working.
    pub previous_token_expires_at: Option<DateTime<Utc>>,
    pub replicas: Vec<Replica>,
}

#[derive(Serialize, ToSchema)]
pub struct RunnerList {
    pub items: Vec<RunnerSummary>,
}

/// One relay replica seen by the API (P4-PRV-2).
#[derive(Serialize, ToSchema)]
pub struct RelayReplicaView {
    pub id: Uuid,
    pub version: String,
    pub streams_open: i32,
    pub last_seen_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
pub struct RelayReplicaList {
    pub replicas: Vec<RelayReplicaView>,
}

/// Returned once on provisioning and rotation.
#[derive(Serialize, ToSchema)]
pub struct RunnerWithToken {
    pub runner: RunnerSummary,
    /// `aur_…`, shown once.
    pub token: String,
}

/// (FIX-025, C-586) A start failure a runner reported or the API's readiness deadline recorded.
#[derive(Serialize, ToSchema)]
pub struct RunnerStartFailure {
    pub id: Uuid,
    pub sandbox_id: Uuid,
    pub agent_id: Uuid,
    /// The failure code (`10 §Failure semantics`; e.g. `supervisor_unreachable`).
    pub code: String,
    /// Human reason, sanitised (never a secret).
    pub message: String,
    /// The start attempt's last phase.
    pub phase: Option<String>,
    /// Warning events of the pod / home claim, newest first.
    pub events: Vec<String>,
    /// Seconds the attempt ran before it failed.
    pub elapsed_secs: Option<i32>,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
pub struct RunnerStartFailures {
    pub items: Vec<RunnerStartFailure>,
}
