use crate::domains::apps::setup::SetupNeed;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

/// One person's ask: their display name and where they asked from. Never an e-mail.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Asker {
    pub user_id: Uuid,
    pub display_name: String,
    pub workspace_id: Uuid,
    pub asked_at: DateTime<Utc>,
    /// (P5-REFINE-8, C-1081) The asker's own "Why do you need it?": plain text, at most 300 characters.
    /// Owners, admins and instance admins see every asker's; a member sees only their own ask.
    pub reason: Option<String>,
}

/// (P5-REFINE-8, C-1080) The level that can resolve a request; computed on read, never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RequestLevel {
    Organization,
    Administration,
}

/// (P5-REFINE-8, C-1081) Who closed a request, as a role (D-407: never a name for the asker).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClosedByRole {
    OrgAdmin,
    InstanceAdmin,
    System,
}

impl ClosedByRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OrgAdmin => "org_admin",
            Self::InstanceAdmin => "instance_admin",
            Self::System => "system",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "org_admin" => Some(Self::OrgAdmin),
            "instance_admin" => Some(Self::InstanceAdmin),
            "system" => Some(Self::System),
            _ => None,
        }
    }
}

/// (C-1081) The person who declined, by display name only.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ClosedBy {
    pub display_name: String,
}

/// An organization's request for an app to be made ready (C-1030).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AppSetupRequest {
    pub id: Uuid,
    pub org_id: Uuid,
    /// (C-1047) `app` | `chat_platform` | `ai_key`.
    pub subject: String,
    /// The app id for `app`; `""` for the other subjects.
    pub app_id: String,
    /// `chat_platform` only: the connector type asked for.
    pub connector_type: Option<String>,
    /// The app's name, or the subject's label ("A chat platform", a connector's name, "An AI key").
    pub app_name: String,
    /// `open` | `resolved` | `declined` | `withdrawn`.
    pub status: String,
    /// The askers; a person who is not an owner or admin of the organization sees only their own ask.
    pub askers: Vec<Asker>,
    /// What is missing (C-1029); empty when the app is switched off.
    pub needs: Vec<SetupNeed>,
    /// The app is switched off for the organization.
    pub unavailable: bool,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    /// (P5-REFINE-8, C-1080) The level that can resolve it.
    pub level: RequestLevel,
    /// (C-1081) The decliner's message to the askers: plain text, at most 300 characters.
    pub message: Option<String>,
    /// (C-1081) Who closed it, as a role.
    pub closed_by_role: Option<ClosedByRole>,
    /// (C-1081) The decliner, for owners, admins and instance admins of the level that declined it only;
    /// never for the asker.
    pub closed_by: Option<ClosedBy>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ListQuery {
    /// `open` (default) | `resolved` | `declined` | `withdrawn` | `history` (closed `resolved` or
    /// `declined` in the last 30 days, newest first, every level; C-1081).
    pub status: Option<String>,
}

/// (C-1081) `GET /workspaces/{ws}/requests` and `GET /admin/requests`.
#[derive(Debug, Deserialize, IntoParams)]
pub struct OpenOrHistory {
    /// `open` (default) | `history` (the last 30 days).
    pub status: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AppSetupRequestList {
    pub items: Vec<AppSetupRequest>,
}

/// `AppSummary.readiness.requested` (C-1029): whether the caller asked, and how many people did.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Requested {
    pub by_me: bool,
    pub count: i64,
}

/// `POST /workspaces/{ws}/requests` (C-1047).
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewRequest {
    /// `app` | `chat_platform` | `ai_key`.
    pub subject: String,
    /// Subject `app`: the catalogue app id (required).
    pub app_id: Option<String>,
    /// Subject `chat_platform`: the connector type id (optional).
    pub connector_type: Option<String>,
    /// (C-1081) Why the person needs it: plain text, 1..300 characters after trimming (optional).
    pub reason: Option<String>,
}

/// (C-1081) The optional body of `POST /workspaces/{ws}/apps/{app}/setup-request`.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AskBody {
    /// Why the person needs it: plain text, 1..300 characters after trimming.
    pub reason: Option<String>,
}

/// (C-1081) The optional body of a decline.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DeclineBody {
    /// A message the askers read with the notice: plain text, 1..300 characters after trimming.
    pub message: Option<String>,
}
