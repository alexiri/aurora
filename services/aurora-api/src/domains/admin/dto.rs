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

/// `GET /v1/admin/identity` (C-511): the Helm-configured role mapping, read-only, no secrets.
#[derive(Serialize, ToSchema)]
pub struct AdminIdentity {
    pub providers: Vec<IdentityProvider>,
}

#[derive(Serialize, ToSchema)]
pub struct IdentityProvider {
    pub id: String,
    pub display_name: Option<String>,
    pub issuer: String,
    /// Dot path of the role claim; `null` when the provider reads no roles.
    pub role_claim: Option<String>,
    /// `id_token` | `userinfo`.
    pub role_claim_source: String,
    pub require_role: bool,
    pub role_mappings: Vec<IdentityMapping>,
}

#[derive(Serialize, ToSchema)]
pub struct IdentityMapping {
    pub value: String,
    pub instance_admin: bool,
    pub org: Option<IdpOrgGrant>,
    pub workspaces: Vec<IdpWorkspaceGrant>,
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

#[derive(Deserialize, ToSchema)]
pub struct PatchAdminUserRequest {
    pub is_instance_admin: Option<bool>,
    pub disabled: Option<bool>,
}
