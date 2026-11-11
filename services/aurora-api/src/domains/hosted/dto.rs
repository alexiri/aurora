use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

/// (phase 4, P4-HAR, C-420) One harness an agent may use.
#[derive(Serialize, ToSchema)]
pub struct HarnessView {
    /// `claude-code` | `codex` (| `fake` when enabled).
    pub id: String,
    pub display_name: String,
    /// Declared dialects, in preference order.
    pub dialects: Vec<String>,
    /// At least one live runner replica lists the harness (its image contains the runtime).
    pub available: bool,
}

#[derive(Serialize, ToSchema)]
pub struct HarnessList {
    pub items: Vec<HarnessView>,
    /// The harness a new hosted agent gets (`hosted.default_harness`).
    pub default: String,
}

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
    /// (P4-PRV-1) The active preview share for this port, when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared: Option<Uuid>,
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
    /// (FIX-025, C-584) The last start attempt, when there was one: when it was claimed, its last
    /// reported phase and detail, the pod's Warning events (bounded) and the runner's last
    /// progress report. Reset by every new boot.
    pub start_attempt: Option<SandboxStartAttempt>,
    /// (FIX-029, C-611) The boot's start log, newest first; true when the state is `failed` and
    /// the failed start may be retried (C-610).
    pub retryable: bool,
    /// (FIX-029, C-611) The boot's start log (phases, the failure verdict, retry notes), newest
    /// first. Reset by every new boot.
    pub start_log: Vec<StartLogEntry>,
    pub processes: Vec<SandboxProcess>,
    pub ports: Vec<SandboxPort>,
    /// (P4-PRV-1, D-145) `preview` is among `keep_awake_reasons` while this is true
    /// ("Preview in use").
    pub preview_in_use: bool,
    pub restart_to_update: bool,
}

/// (FIX-029, C-611) One entry of the boot's start log (`sandboxes.start_log`, C-612).
#[derive(Serialize, ToSchema)]
pub struct StartLogEntry {
    pub ts: DateTime<Utc>,
    /// `phase` (a runner phase report or the failure verdict) or `note` (a retry).
    pub kind: String,
    /// The phase the entry records (`starting`, `booting`, `failed`, `retry`…).
    pub phase: Option<String>,
    /// The entry's detail, sanitised at write (≤ 300 chars).
    pub detail: Option<String>,
}

/// (FIX-025, C-584) One start attempt of the agent's computer (Computer tab).
#[derive(Serialize, ToSchema)]
pub struct SandboxStartAttempt {
    /// When the API claimed the start (the attempt's clock origin).
    pub started_at: DateTime<Utc>,
    /// Last phase the runner reported (`claimed`, `waiting_capacity`, `provisioning`,
    /// `scheduling`, `creating`, `booting`, `ready`…).
    pub phase: Option<String>,
    /// The phase's detail as the runner reported it (sanitised; e.g. the scheduler's message).
    pub detail: Option<String>,
    /// Warning events of the pod and the home claim, newest first.
    pub events: Vec<String>,
    /// When the runner last reported progress.
    pub progress_at: Option<DateTime<Utc>>,
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

/// (FIX-029, C-613/C-614) One start failure of an agent's sandbox — the C-586 shape; the admin
/// route adds `runner_id`.
#[derive(Serialize, ToSchema)]
pub struct AgentStartFailure {
    pub id: Uuid,
    pub sandbox_id: Uuid,
    /// The runner that hosted the failed attempt (C-614 admin route only; `null` for a legacy
    /// row whose runner was deleted).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner_id: Option<Uuid>,
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
pub struct AgentStartFailures {
    pub items: Vec<AgentStartFailure>,
}
