//! API-006 wire types (docs/API_CONTRACTS.md §M6).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationStatus {
    Validating,
    Active,
    NeedsAttention,
    Disabled,
}

impl IntegrationStatus {
    pub fn parse(s: &str) -> Self {
        match s {
            "active" => Self::Active,
            "needs_attention" => Self::NeedsAttention,
            "disabled" => Self::Disabled,
            _ => Self::Validating,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SharedBotInfo {
    pub handle: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ChannelIntegration {
    pub id: Uuid,
    pub org_id: Uuid,
    pub connector_id: String,
    pub tenant_ref: String,
    pub tenant_label: String,
    /// Connector configuration without credentials; `_capabilities` holds the narrowing
    /// reported by the connector.
    #[schema(value_type = Object)]
    pub config: serde_json::Value,
    pub shared_bot: Option<SharedBotInfo>,
    pub status: IntegrationStatus,
    pub status_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateIntegrationRequest {
    pub connector_id: String,
    /// Non-secret settings from the connector's `integration_setup` form.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub config: serde_json::Value,
    /// Platform credentials. Write-only: never returned.
    #[schema(value_type = Object)]
    pub credentials: serde_json::Value,
}

/// `POST /orgs/{org}/channel-integrations:probe` body (C-711): the create shapes.
#[derive(Deserialize, ToSchema)]
pub struct ProbeIntegrationRequest {
    pub connector_id: String,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub config: serde_json::Value,
    /// Write-only: used once, never stored or logged.
    #[schema(value_type = Object)]
    pub credentials: serde_json::Value,
}

#[derive(Serialize, ToSchema)]
pub struct ProbeChoice {
    pub value: String,
    pub label: String,
}

#[derive(Serialize, ToSchema)]
pub struct ProbeIntegrationResponse {
    /// Per setup field that declares `choices: "from_validate"`.
    pub choices: std::collections::BTreeMap<String, Vec<ProbeChoice>>,
    pub tenant_label: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct PatchIntegrationRequest {
    /// Replaces the whole config (re-validated).
    #[schema(value_type = Option<Object>)]
    pub config: Option<serde_json::Value>,
    /// Replaces the credentials (re-validated). Write-only.
    #[schema(value_type = Option<Object>)]
    pub credentials: Option<serde_json::Value>,
    pub status: Option<SettableStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SettableStatus {
    Active,
    Disabled,
}

impl SettableStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PresenceMode {
    Dedicated,
    Shared,
}

impl PresenceMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dedicated => "dedicated",
            Self::Shared => "shared",
        }
    }
    pub fn parse(s: &str) -> Self {
        if s == "shared" {
            Self::Shared
        } else {
            Self::Dedicated
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PresenceStatus {
    PendingSetup,
    Active,
    Disabled,
    NeedsAttention,
}

impl PresenceStatus {
    pub fn parse(s: &str) -> Self {
        match s {
            "active" => Self::Active,
            "disabled" => Self::Disabled,
            "needs_attention" => Self::NeedsAttention,
            _ => Self::PendingSetup,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Presence {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub integration_id: Uuid,
    pub connector_id: String,
    pub tenant_label: String,
    pub mode: PresenceMode,
    pub handle: Option<String>,
    pub status: PresenceStatus,
    pub status_reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// One integration an agent may attach a presence to (C-39): metadata only.
#[derive(Serialize, ToSchema)]
pub struct PresenceOption {
    pub integration_id: Uuid,
    pub connector_id: String,
    pub connector_name: String,
    pub tenant_label: String,
    pub modes: Vec<PresenceMode>,
    /// `{ dedicated?: { fields: FormField[] } }`.
    #[schema(value_type = Object)]
    pub presence_setup: serde_json::Value,
    /// The agent already has a presence on this integration.
    pub attached: bool,
}

#[derive(Serialize, ToSchema)]
pub struct PresenceOptionList {
    pub items: Vec<PresenceOption>,
}

#[derive(Serialize, ToSchema)]
pub struct PresenceList {
    pub items: Vec<Presence>,
}

#[derive(Deserialize, ToSchema)]
pub struct AttachPresenceRequest {
    pub integration_id: Uuid,
    pub mode: PresenceMode,
    /// Values for the connector's `presence_setup.dedicated.fields` (manual setup).
    #[serde(default)]
    pub fields: Option<BTreeMap<String, String>>,
}

#[derive(Deserialize, ToSchema, Default)]
pub struct ReattachRequest {
    /// Values for the connector's `presence_setup.dedicated.fields` (manual setup).
    #[serde(default)]
    pub fields: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Listen {
    Mentions,
    All,
}

impl Listen {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mentions => "mentions",
            Self::All => "all",
        }
    }
    pub fn parse(s: &str) -> Self {
        if s == "all" {
            Self::All
        } else {
            Self::Mentions
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct UserRef {
    pub id: Uuid,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SpaceBinding {
    pub id: Uuid,
    pub integration_id: Uuid,
    pub space_ref: String,
    pub space_label: String,
    pub agent_id: Uuid,
    pub listen: Listen,
    pub followups_need_mention: bool,
    pub active: bool,
    pub bound_by: Option<UserRef>,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
pub struct SpaceBindingList {
    pub items: Vec<SpaceBinding>,
}

/// `POST /agents/{agent}/space-bindings` (C-713).
#[derive(Deserialize, ToSchema)]
pub struct CreateBindingRequest {
    pub integration_id: Uuid,
    pub space_ref: String,
    /// Default `mentions`.
    pub listen: Option<Listen>,
}

/// `GET /integrations/{id}/spaces` query (C-714).
#[derive(Deserialize, utoipa::IntoParams)]
pub struct MySpacesQuery {
    /// Must be `true`.
    pub mine: Option<bool>,
    pub query: Option<String>,
    /// 1..=50, default 25.
    pub limit: Option<u32>,
}

#[derive(Serialize, ToSchema)]
pub struct MySpace {
    pub space_ref: String,
    pub label: String,
    /// `channel` or `private`.
    pub kind: String,
    pub bot_present: bool,
}

#[derive(Serialize, ToSchema)]
pub struct MySpacesList {
    pub items: Vec<MySpace>,
}

#[derive(Deserialize, ToSchema)]
pub struct PatchBindingRequest {
    pub listen: Option<Listen>,
    pub followups_need_mention: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkMethod {
    VerifiedEmail,
    SignInLink,
    Admin,
}

impl LinkMethod {
    pub fn parse(s: &str) -> Self {
        match s {
            "verified_email" => Self::VerifiedEmail,
            "sign_in_link" => Self::SignInLink,
            _ => Self::Admin,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct UserLink {
    pub id: Uuid,
    pub integration_id: Uuid,
    pub connector_id: String,
    pub tenant_label: String,
    pub platform_user_ref: String,
    pub user: UserRef,
    pub method: LinkMethod,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
pub struct UserLinkList {
    pub items: Vec<UserLink>,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateUserLinkRequest {
    pub platform_user_ref: String,
    pub user_id: Uuid,
}

#[derive(Deserialize, ToSchema)]
pub struct ConfirmLinkRequest {
    /// The `aui_…` token from the sign-in link.
    pub token: String,
    /// The platform user the confirm page showed (`LinkPreview.platform_user_ref`); the request is
    /// refused (422 `link_reference_mismatch`) when it is not the token's chat account.
    pub platform_user_ref: String,
}

/// `GET /channel-links/preview?token=`: the chat account a sign-in link would bind (docs/13 §5.9).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct LinkPreview {
    pub integration_id: Uuid,
    pub connector_id: String,
    pub tenant_label: String,
    /// The platform's reference for the person (what the confirm request repeats).
    pub platform_user_ref: String,
    /// What the platform called the person when the link was requested, if known.
    pub platform_display_name: Option<String>,
    /// The email the platform asserted, if any (a hint, never an authorization).
    pub platform_email_hint: Option<String>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct LinkPreviewParams {
    /// The `aui_…` token from the sign-in link.
    pub token: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DmDefault {
    pub integration_id: Uuid,
    pub platform_user_ref: String,
    pub agent_id: Option<Uuid>,
}

#[derive(Serialize, ToSchema)]
pub struct DmDefaultList {
    pub items: Vec<DmDefault>,
}

#[derive(Deserialize, ToSchema)]
pub struct SetDmDefaultRequest {
    pub integration_id: Uuid,
    pub agent_id: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    Space,
    Person,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReachState {
    Pending,
    Approved,
    Blocked,
    Left,
    Expired,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ReachGrant {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub integration_id: Uuid,
    pub subject_kind: SubjectKind,
    pub subject_ref: String,
    pub subject_label: String,
    pub state: ReachState,
    pub listen: Listen,
    pub revision: i64,
    pub decided_by: Option<UserRef>,
    pub decided_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReachDecision {
    Approve,
    Block,
}

#[derive(Deserialize, ToSchema)]
pub struct ReachDecisionRequest {
    pub decision: ReachDecision,
    /// Defaults to the grant's current listen mode.
    pub listen: Option<Listen>,
    pub revision: i64,
    /// (FIX-111) Approve only: the grant expires this many seconds from now (60 s to 365 days)
    /// and then needs a new approval; without it a grant does not expire.
    pub expires_in: Option<u32>,
}
