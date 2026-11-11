//! Preview share DTOs (phase 4, P4-PRV-1, D-144, D-145; C-540..C-549).
use aurora_db::repo::preview_shares::ShareRow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// `private`: the only visibility in phase 4 (a live dashboard session or an API key of someone
/// for whom `can_use(workspace)` holds; never an agent credential, docs/04 §Preview access).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PreviewVisibility {
    Private,
}

/// `active` | `paused` (stored state; the displayed `status` is derived, not stored).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PreviewState {
    Active,
    Paused,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewCreatedBy {
    pub id: Option<Uuid>,
    pub label: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewLastStart {
    pub at: DateTime<Utc>,
    pub exit_code: Option<i32>,
    pub tail: Option<String>,
}

/// One preview share of a hosted agent: `https://<label>.<previews.domain>/`.
#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewShare {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub port: i32,
    /// Hostname label, `p` + 24 base32 lowercase chars; stable for the share's life, never reused.
    pub label: String,
    /// `https://<label>.<previews.domain>/`
    pub hostname: String,
    pub url: String,
    pub visibility: PreviewVisibility,
    pub state: PreviewState,
    /// `listening` | `not_listening` | `waking` | `paused` | `frozen` (derived, docs/05).
    pub status: String,
    pub start_command: Option<String>,
    pub cwd: Option<String>,
    pub last_start: Option<PreviewLastStart>,
    pub last_accessed_at: Option<DateTime<Utc>>,
    pub created_by: PreviewCreatedBy,
    pub created_at: DateTime<Utc>,
}

/// Share row + the derived status (needs the sandbox and the freeze state).
pub struct ShareView {
    pub row: ShareRow,
    pub status: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewShareList {
    pub shares: Vec<PreviewShare>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewStartAccepted {
    /// `starting` (the demand was recorded; the sandbox boots or the command launches).
    pub status: String,
}

fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreatePreviewShareRequest {
    /// TCP port of the agent's sandbox, 1024..65535, not one of the supervisor's listeners.
    pub port: i32,
    /// Optional on-demand start command (one line, ≤ 1024 chars).
    pub start_command: Option<String>,
    /// Optional working directory relative to `/workspace` (no `..`, ≤ 200 chars).
    pub cwd: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PatchPreviewShareRequest {
    /// `active` | `paused`.
    pub state: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<String>)]
    pub start_command: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<String>)]
    pub cwd: Option<Option<String>>,
}
