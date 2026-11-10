use crate::domains::admin::dto::IdpGrant;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// `GET/PATCH /v1/me/onboarding` (C-530): what cannot be computed from real state.
#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct MeOnboarding {
    /// The Welcome page was opened (or skipped). `false` for a person who has not seen it.
    pub welcome_seen: bool,
    /// Workspaces whose getting-started guide the person dismissed.
    pub guide_dismissed: Vec<Uuid>,
    /// When the navigation tour was finished or skipped; `null` = not yet.
    pub tour_completed_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize, ToSchema, Debug)]
#[serde(deny_unknown_fields)]
pub struct PatchOnboarding {
    pub welcome_seen: Option<bool>,
    /// The whole list (at most 200 workspaces). `[]` shows every guide again.
    pub guide_dismissed: Option<Vec<Uuid>>,
    /// `true` records now; `false` clears it so the tour is offered again.
    pub tour_completed: Option<bool>,
}

/// Status of one setup step or check.
#[derive(Serialize, ToSchema, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum SetupState {
    Ok,
    Warning,
    Todo,
}

/// `GET /v1/admin/setup` (C-533): the setup wizard's live status, no secrets.
#[derive(Serialize, ToSchema)]
pub struct SetupStatus {
    /// A placement has been saved: the wizard is no longer shown at sign-in.
    pub placement_saved: bool,
    /// The calling admin's last sign-in, when it read a role claim.
    pub you: Option<SetupYou>,
    /// In order: `roles`, `placement`, `llm`, `runners`, `optional`. ("Try it" is the client's.)
    pub steps: Vec<SetupStep>,
}

#[derive(Serialize, ToSchema)]
pub struct SetupYou {
    pub provider: String,
    pub provider_name: Option<String>,
    /// The claim path the provider reads, when it reads one.
    pub role_claim: Option<String>,
    /// Every role value in the token, mapped or not.
    pub values: Vec<String>,
    pub granted: Vec<IdpGrant>,
}

#[derive(Serialize, ToSchema)]
pub struct SetupStep {
    pub id: String,
    pub status: SetupState,
    /// Machine-readable reason of the worst check (see `API_CONTRACTS` C-533).
    pub reason: String,
    pub checks: Vec<SetupCheck>,
}

#[derive(Serialize, ToSchema)]
pub struct SetupCheck {
    /// Stable id, e.g. `endpoint:litellm`, `default_model:litellm`, `gateway`.
    pub id: String,
    pub status: SetupState,
    pub reason: String,
    /// One short plain sentence for the page.
    pub message: String,
    /// The current value (never a secret): a model id, an image, a count.
    pub value: Option<String>,
    /// The Helm value that changes it, for config-only things.
    pub helm: Option<String>,
}
