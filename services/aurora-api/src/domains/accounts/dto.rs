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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    Ok,
    Rejected,
    SignInExpired,
    Unreachable,
    Disabled,
    Unknown,
}

/// C-944. Until P5-UX-14 adds the check fields: `sign_in_expired`, `disabled`, else `unknown`.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Health {
    pub state: HealthState,
    pub detail: Option<String>,
    pub checked_at: Option<DateTime<Utc>>,
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
pub enum AccessLevel {
    Read,
    Ask,
    Act,
    Custom,
    Use,
}

impl From<Level> for AccessLevel {
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
    pub level: AccessLevel,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connected_by: Option<PersonName>,
    pub agents: Vec<AgentAccess>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub managed_by: Option<WhoCan>,
    pub can: AccountCan,
    /// Only with `agent_id`: whether that agent can use the account (given ones come first).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub given: Option<bool>,
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
