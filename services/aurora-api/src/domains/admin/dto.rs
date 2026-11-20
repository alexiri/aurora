use crate::domains::auth::dto::User;
use aurora_db::repo::account::AdminUserRow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
pub struct AdminUser {
    #[serde(flatten)]
    pub user: User,
    pub disabled_at: Option<DateTime<Utc>>,
    pub last_login_at: Option<DateTime<Utc>>,
    /// (phase 4, P4-IDP-1, C-512) The IdP role value that gives the instance-admin flag, or
    /// `null` when it is manual. A claim-derived flag cannot be changed here (409 `managed_by_idp`).
    pub instance_admin_idp_value: Option<String>,
    /// (phase 4, P4-IDP-1, C-512) The claim debugger: values of the last sign-in and what they
    /// granted; `null` before a sign-in that read a role claim.
    pub idp_roles: Option<IdpRoles>,
}

/// What one claim value granted (a `role_mappings[]` entry that matched).
#[derive(Serialize, Deserialize, ToSchema, Clone)]
pub struct IdpGrant {
    pub value: String,
    pub instance_admin: bool,
    pub org: Option<IdpOrgGrant>,
    pub workspaces: Vec<IdpWorkspaceGrant>,
}

#[derive(Serialize, Deserialize, ToSchema, Clone)]
pub struct IdpOrgGrant {
    pub slug: String,
    /// `owner` | `admin` | `member`.
    pub role: String,
}

#[derive(Serialize, Deserialize, ToSchema, Clone)]
pub struct IdpWorkspaceGrant {
    pub slug: String,
    /// `owner` | `approver` | `member`.
    pub role: String,
}

/// Role values of a user's last sign-in and what they mapped to (C-512, C-513).
#[derive(Serialize, ToSchema, Clone)]
pub struct IdpRoles {
    pub provider: String,
    /// Every value the claim carried, mapped or not.
    pub values: Vec<String>,
    pub synced_at: DateTime<Utc>,
    pub granted: Vec<IdpGrant>,
}

impl From<aurora_db::repo::idp_roles::SyncState> for IdpRoles {
    fn from(s: aurora_db::repo::idp_roles::SyncState) -> Self {
        Self {
            provider: s.provider_id,
            values: s.claim_values,
            synced_at: s.synced_at,
            granted: serde_json::from_value(s.granted).unwrap_or_default(),
        }
    }
}

impl From<AdminUserRow> for AdminUser {
    fn from(u: AdminUserRow) -> Self {
        Self {
            user: User {
                id: u.id,
                email: u.email,
                display_name: u.display_name,
                email_verified_at: u.email_verified_at,
                is_instance_admin: u.is_instance_admin,
                created_at: u.created_at,
            },
            disabled_at: u.disabled_at,
            last_login_at: u.last_login_at,
            instance_admin_idp_value: u.idp_admin_value,
            idp_roles: None,
        }
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListUsersQuery {
    /// Case-insensitive substring of email or display name.
    pub q: Option<String>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

/// (FIX-167 Part B, C-1022) What an item of `GET /v1/admin/attention` is about.
#[derive(Serialize, ToSchema, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    /// A connector waiting for its first approval, or running while an update waits.
    ConnectorApproval,
    /// A package version in `pending_approval`.
    PackageApproval,
    /// An add-on source whose signing key changed.
    AddonKeyChanged,
    /// Two packages claim the same host; one waits for a winner.
    HostClaimConflict,
    /// A managed connector whose hosts report `error` or that no host reports (not a decision).
    ConnectorDown,
    /// An organization without an active owner (not a decision).
    OwnerlessOrg,
    /// (P5-REFINE-8, C-1080) People asked for an app (organization or Administration level).
    AppRequest,
    /// (C-1080) People asked for a chat platform.
    ChatPlatformRequest,
    /// (C-1080) People asked for an AI key (organization level).
    AiKeyRequest,
    /// (C-1080) A pending gateway or action approval the caller may decide (workspace level).
    Approval,
    /// (C-1080) An agent asked for an app or a website's token (workspace level).
    AccessRequest,
    /// (C-1080) An account of the workspace that is failing or must sign in again.
    BrokenAccount,
    /// (C-1080) The caller's own waiting request (`own: true`, never counted).
    MyRequest,
}

impl AttentionKind {
    /// Decision items are e-mailed and pushed once when they first appear.
    pub fn is_decision(self) -> bool {
        !matches!(
            self,
            Self::ConnectorDown | Self::OwnerlessOrg | Self::BrokenAccount | Self::MyRequest
        )
    }
}

/// (C-1080) The level an item belongs to.
#[derive(Serialize, ToSchema, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum AttentionLevel {
    Workspace,
    Organization,
    Administration,
}

/// (C-1080) `open`, or for `state=done`: `done` (resolved, decided) or `declined`.
#[derive(Serialize, ToSchema, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum AttentionState {
    Open,
    Done,
    Declined,
}

/// (C-1080) A person behind an item: display name only, never an e-mail.
#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct AttentionActor {
    pub user_id: Uuid,
    pub display_name: String,
}

/// (C-1080) The one primary action.
#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct AttentionAction {
    pub label: String,
    pub href: String,
}

/// One thing that needs someone at a level: a fixed sentence filled with names (C-1022, C-1080).
#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct AttentionItem {
    /// (C-1080) Stable: the request id for request kinds, `<kind>:<subject_id>` for the others.
    pub id: String,
    pub kind: AttentionKind,
    /// (C-1080)
    pub level: AttentionLevel,
    /// (C-1080) The rail's page that resolves it: `chat-platforms`, `apps`, `ai-models`, `people`,
    /// `approvals`, `accounts`.
    pub topic: String,
    /// The connector id, the package or source id, the organization slug, the request, approval,
    /// access request or account id.
    pub subject_id: String,
    pub title: String,
    pub detail: String,
    /// When the item first appeared (or the row last changed, for an item that predates the notice table).
    pub since: DateTime<Utc>,
    /// The dashboard page that resolves it (equals `action.href` when there is an action).
    pub href: String,
    /// The administrator has to decide (approve, reject, choose); informational items are `false`.
    pub decision: bool,
    /// (C-1080) The askers (at most 5, oldest first); empty for kinds without people.
    pub actors: Vec<AttentionActor>,
    /// (C-1080) How many askers in all.
    pub actors_total: i64,
    /// (C-1080, C-1081) The first asker's reason (plain text, at most 300 characters); null otherwise.
    pub reason: Option<String>,
    /// (C-1080) The caller's own request (`my_request`); never counted.
    pub own: bool,
    /// (C-1080) The one primary action; null when the caller cannot act ("Ask <role>" is in `detail`).
    pub action: Option<AttentionAction>,
    /// (C-1080) The caller may decline it (request kinds at the caller's level).
    pub can_decline: bool,
    /// (C-1080)
    pub state: AttentionState,
    /// (C-1080) `state=done`: when it closed.
    pub closed_at: Option<DateTime<Utc>>,
    /// (C-1080, C-1081) The decline message (plain text, at most 300 characters).
    pub message: Option<String>,
    /// (C-1080, C-1081) Who closed it, as a role.
    pub closed_by_role: Option<crate::domains::app_setup_requests::dto::ClosedByRole>,
    /// (C-1080) The decliner, only for viewers who may act at that level; never for the asker.
    pub closed_by: Option<crate::domains::app_setup_requests::dto::ClosedBy>,
}

impl AttentionItem {
    /// An open item with no people, no action and no history; the builders fill in the rest.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kind: AttentionKind,
        level: AttentionLevel,
        topic: &str,
        subject_id: String,
        text: (String, String),
        since: DateTime<Utc>,
        href: String,
    ) -> Self {
        Self {
            id: String::new(),
            decision: kind.is_decision(),
            kind,
            level,
            topic: topic.to_owned(),
            subject_id,
            title: text.0,
            detail: text.1,
            since,
            href,
            actors: Vec::new(),
            actors_total: 0,
            reason: None,
            own: false,
            action: None,
            can_decline: false,
            state: AttentionState::Open,
            closed_at: None,
            message: None,
            closed_by_role: None,
            closed_by: None,
        }
        .with_default_id()
    }

    fn with_default_id(mut self) -> Self {
        let k = serde_json::to_value(self.kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        self.id = format!("{k}:{}", self.subject_id);
        self
    }

    /// Sets the primary action; `href` follows it.
    pub fn action(mut self, label: &str, href: String) -> Self {
        self.href.clone_from(&href);
        self.action = Some(AttentionAction {
            label: label.to_owned(),
            href,
        });
        self
    }
}

/// `GET /v1/admin/attention` (C-1022), `GET /v1/workspaces/{ws}/attention` and `GET /v1/orgs/{org}/attention`
/// (C-1080): decisions first, oldest first; `state=done` newest first; computed on read.
#[derive(Serialize, ToSchema)]
pub struct AttentionList {
    pub items: Vec<AttentionItem>,
    /// (C-1080) What the rail shows: the open items with `own: false`.
    pub count: i64,
}

/// (C-1080) `?state=open|done`.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AttentionQuery {
    /// `open` (default) | `done` (closed in the last 30 days).
    pub state: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct PatchAdminUserRequest {
    pub is_instance_admin: Option<bool>,
    pub disabled: Option<bool>,
}
