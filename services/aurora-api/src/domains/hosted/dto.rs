use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
pub struct SandboxProcess {
    pub id: String,
    pub name: String,
    pub state: String,
    pub busy: bool,
    pub started_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, ToSchema)]
pub struct SandboxPort {
    pub port: i32,
    pub protocol: String,
}

#[derive(Serialize, ToSchema)]
pub struct SandboxView {
    pub agent_id: Uuid,
    /// unprovisioned | starting | running | stopping | stopped | failed
    pub state: String,
    pub boot_id: Option<Uuid>,
    pub runner_id: Option<Uuid>,
    pub image: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub keep_awake_reasons: Vec<String>,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
    pub processes: Vec<SandboxProcess>,
    pub ports: Vec<SandboxPort>,
    pub restart_to_update: bool,
}

#[derive(Serialize, ToSchema)]
pub struct ModelOption {
    pub endpoint_id: String,
    pub provider: String,
    pub model_id: String,
    pub display_name: Option<String>,
    pub dialects: Vec<String>,
}

/// One granted LLM key and whether the dispatcher can use it (C-500).
#[derive(Serialize, ToSchema)]
pub struct KeyReadiness {
    pub secret_id: Uuid,
    /// The key's name; shown to workspace owners only (this route is `can_own`).
    pub name: String,
    /// `organization` | `workspace`.
    pub scope: String,
    pub endpoint_id: String,
    pub provider: String,
    pub usable: bool,
    /// First failing check when not usable: `not_active` | `endpoint_unknown` |
    /// `provider_disabled` | `scope_not_permitted` | `origin_unverified` |
    /// `origin_not_compliant` | `no_shared_dialect`; null when usable.
    pub reason: Option<String>,
}

/// The model the next turn would use without further input (C-500).
#[derive(Serialize, ToSchema)]
pub struct DefaultModel {
    pub endpoint_id: String,
    pub provider: String,
    pub model_id: String,
    /// `override` | `endpoint_default` | `provider_default` | `allow_list` | `static_list`.
    pub source: String,
}

#[derive(Serialize, ToSchema)]
pub struct ModelReadiness {
    pub harness: String,
    /// In the order the dispatcher considers them.
    pub keys: Vec<KeyReadiness>,
    /// Null when no model resolves (`no_default_model`) or no key is usable.
    pub default: Option<DefaultModel>,
    /// An agent model override exists but is not usable (the next turn drops it with a notice).
    pub override_dropped: bool,
}

#[derive(Serialize, ToSchema)]
pub struct ModelOptions {
    pub items: Vec<ModelOption>,
}
