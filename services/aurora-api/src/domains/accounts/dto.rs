//! (P5-UX-4, C-917) The accounts lists: one shape for apps, websites and AI keys.

use crate::domains::apps::dto::AppFamily;
use crate::domains::grants::levels::Level;
use crate::http::who_can::WhoCan;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccountRefType {
    Connection,
    Secret,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AccountRef {
    #[serde(rename = "type")]
    pub kind: AccountRefType,
    pub id: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    App,
    Website,
    AiKey,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AccountApp {
    pub app_id: String,
    pub name: String,
    pub family: Option<AppFamily>,
    /// The app allows custom access only (C-410): no `act` choice, the actions table opens
    /// directly.
    pub custom_only: bool,
    /// (P5-REFINE-6, C-1067) The app only sends (no read tool): `levels` are `ask` and `act`.
    pub send_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    /// Status `reauth_required`.
    SignInExpired,
    /// Status `disabled`.
    Disabled,
    /// An agent's call failed (`last_error_at`) and none has succeeded since.
    Failing,
    /// An agent's call succeeded (`last_ok_at`) after the last failure, if any.
    Working,
    /// No agent's call has shown anything yet. A Test never changes this (C-1033).
    NotUsed,
}

/// What the last Test said (C-1033). Shown as "Test passed … ago", never as the account working.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct HealthTest {
    pub at: DateTime<Utc>,
    /// `ok` | `rejected` | `unreachable` | `unexpected`.
    pub result: String,
}

/// (C-1065) Whether an AI key works, as far as is known. Advisory: Test is always authoritative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum VerificationState {
    /// The origin is `pending` and a background re-check is queued on this replica.
    Checking,
    /// Origin verified, a real call succeeded, or the last Test passed.
    Works,
    /// The last Test or real call says the provider refused the key.
    Rejected,
    /// The last Test found nothing answering.
    Unreachable,
    /// Pending with no re-check queued, or nothing known yet.
    NotChecked,
}

#[derive(Debug, Clone, Serialize, ToSchema, PartialEq, Eq)]
pub struct Verification {
    pub state: VerificationState,
    /// When the evidence behind `state` was recorded; null for `checking` / `not_checked`.
    pub checked_at: Option<DateTime<Utc>>,
}

/// C-944, amended by C-1033: `state` follows the account's status and what agents' real calls showed.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Health {
    pub state: HealthState,
    /// For `failing`: the word of what the app refuses (`token`, `password`, `key`) when `last_error` is
    /// `rejected`; the host when it is `unreachable` (filled in by the caller).
    pub detail: Option<String>,
    /// The last successful agent call through the account.
    pub last_ok_at: Option<DateTime<Utc>>,
    /// The last failed agent call, and why (`rejected | unreachable | unexpected | expired`).
    pub last_error_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    /// The last Test, apart from use.
    pub test: Option<HealthTest>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PersonName {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AccountAgent {
    pub id: Uuid,
    pub name: String,
    pub handle: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AccountWorkspace {
    pub id: Uuid,
    pub name: String,
}

/// What one agent's saved choice reads as. `use` is the level of accounts with no choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentAccessLevel {
    Read,
    Ask,
    Act,
    Custom,
    Use,
}

impl From<Level> for AgentAccessLevel {
    fn from(l: Level) -> Self {
        match l {
            Level::Read => Self::Read,
            Level::Ask => Self::Ask,
            Level::Act => Self::Act,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AlwaysAllowed {
    pub tool_id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CustomChoice {
    /// The nearest level.
    pub base: Level,
    /// How many actions differ from it.
    pub differs: u32,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AgentAccess {
    pub agent: AccountAgent,
    pub workspace: AccountWorkspace,
    pub level: AgentAccessLevel,
    /// Under `ask`: the changes the saved choice allows without asking.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub always_allowed: Option<Vec<AlwaysAllowed>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom: Option<CustomChoice>,
    /// AI-key rows of a list read for one agent (`agent_id`): the model that agent's next turn
    /// uses through the key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
pub struct AccountCan {
    pub give_access: bool,
    pub change_access: bool,
    pub manage: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Account {
    #[serde(rename = "ref")]
    pub reference: AccountRef,
    pub kind: AccountKind,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<AccountApp>,
    /// `workspace` | `organization`.
    pub scope: String,
    /// An organization's account of an app that can change things: the most any agent may do.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<Level>,
    /// The choices that exist for the account, already cut to the limit; empty = no choice.
    pub levels: Vec<Level>,
    pub health: Health,
    /// (FIX-163) Why a turned-off app account is off, when something other than a person's choice turned it
    /// off: `instance_deleted` (its self-hosted app instance was removed; it cannot be turned on again).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connected_by: Option<PersonName>,
    pub agents: Vec<AgentAccess>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub managed_by: Option<WhoCan>,
    pub can: AccountCan,
    /// Only with `agent_id`: whether that agent can use the account (given ones come first).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub given: Option<bool>,
    /// (P5-APIGAP-3, C-1048) An AI key only: the model an agent gets on this key with no override (D-372);
    /// null when unknown and on every other kind.
    pub default_model: Option<String>,
    /// (P5-APIGAP-3, C-1049) An organization's AI key in the organization list only: the instance's starter
    /// grants give it to every new agent of the organization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starter: Option<bool>,
    /// (C-1065) An AI key only: whether it works (omitted on other kinds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<Verification>,
}

/// (C-943) What Test found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccountTestResult {
    Ok,
    Rejected,
    Unreachable,
    Unexpected,
    NotTestable,
}

impl AccountTestResult {
    /// The stored form (`last_test_result`); `not_testable` is never stored.
    pub fn stored(self) -> Option<&'static str> {
        match self {
            Self::Ok => Some("ok"),
            Self::Rejected => Some("rejected"),
            Self::Unreachable => Some("unreachable"),
            Self::Unexpected => Some("unexpected"),
            Self::NotTestable => None,
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "ok" => Self::Ok,
            "rejected" => Self::Rejected,
            "unreachable" => Self::Unreachable,
            "not_testable" => Self::NotTestable,
            _ => Self::Unexpected,
        }
    }
}

/// At most a host, an HTTP status or a protocol code: never an upstream body, header or credential (SEC-129).
#[derive(Debug, Clone, Default, Serialize, ToSchema)]
pub struct AccountTestDetail {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// C-943: the answer of `POST /v1/accounts/{type}/{id}/test`.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AccountTestAnswer {
    pub result: AccountTestResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<AccountTestDetail>,
    pub checked_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct AccountsQuery {
    /// Workspace list only: read the list for this agent (adds `given`, given first, the model
    /// line on AI keys).
    pub agent_id: Option<Uuid>,
    /// `app` | `website` | `ai_key`.
    pub kind: Option<String>,
    /// Only the accounts of this app.
    pub app_id: Option<String>,
}
