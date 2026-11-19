use crate::domains::auth::dto::User;
use aurora_db::repo::account::AdminUserRow;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

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
#[derive(Serialize, ToSchema, Clone, Copy, PartialEq, Eq)]
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
}

impl AttentionKind {
    /// Decision items are e-mailed and pushed once when they first appear.
    pub fn is_decision(self) -> bool {
        !matches!(self, Self::ConnectorDown | Self::OwnerlessOrg)
    }
}

/// One thing that needs an instance admin: a fixed sentence filled with names and versions.
#[derive(Serialize, ToSchema)]
pub struct AttentionItem {
    pub kind: AttentionKind,
    /// The connector id, the package or source id, or the organization slug.
    pub subject_id: String,
    pub title: String,
    pub detail: String,
    /// When the item first appeared (or the row last changed, for an item that predates the notice table).
    pub since: DateTime<Utc>,
    /// The dashboard page that resolves it.
    pub href: String,
    /// The administrator has to decide (approve, reject, choose); informational items are `false`.
    pub decision: bool,
}

/// `GET /v1/admin/attention` (C-1022): decisions first, oldest first; computed on read.
#[derive(Serialize, ToSchema)]
pub struct AttentionList {
    pub items: Vec<AttentionItem>,
}

#[derive(Deserialize, ToSchema)]
pub struct PatchAdminUserRequest {
    pub is_instance_admin: Option<bool>,
    pub disabled: Option<bool>,
}
