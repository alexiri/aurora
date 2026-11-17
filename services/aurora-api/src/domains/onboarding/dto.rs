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
    /// (P5-IA-1, C-987) Keys of the one-time notices the person dismissed, at most 50. A person
    /// created after the 2026-11 release has `renames-2026-11` from the start.
    pub notices_seen: Vec<String>,
}

#[derive(Deserialize, ToSchema, Debug)]
#[serde(deny_unknown_fields)]
pub struct PatchOnboarding {
    pub welcome_seen: Option<bool>,
    /// The whole list (at most 200 workspaces). `[]` shows every guide again.
    pub guide_dismissed: Option<Vec<Uuid>>,
    /// `true` records now; `false` clears it so the tour is offered again.
    pub tour_completed: Option<bool>,
    /// (C-987) Append one notice key (1–64 characters: lowercase letters, digits, `-`).
    pub notice_seen: Option<String>,
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
    /// (C-922) Personal workspaces of the default organization whose default agent lacks a
    /// starter grant of the saved placement: people who signed in before it was set. `0` when
    /// no placement or no starter grant is saved.
    pub personal_workspaces_without_starter: i64,
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
    /// The current value (never a secret): a model id, an image, a count; an app's name for
    /// the `app_*` checks.
    pub value: Option<String>,
    /// The Helm value that changes it, for config-only things.
    pub helm: Option<String>,
    /// (C-924) `app_needs_oauth_client` and `app_token_only`: the app the check is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    /// (C-924) Those checks: the app's package id and the OAuth client key its sign-in method
    /// reads (`null` for `app_token_only` when it has none to name).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_key: Option<String>,
}
