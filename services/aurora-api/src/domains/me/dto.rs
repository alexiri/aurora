use crate::domains::auth::dto::User;
use crate::domains::tenancy::dto::{MemberStatus, OrgRole, OrgSummary, WorkspaceRole};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
pub struct MeWorkspace {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub my_role: WorkspaceRole,
}

#[derive(Serialize, ToSchema)]
pub struct MeMembership {
    pub org: OrgSummary,
    pub role: OrgRole,
    pub status: MemberStatus,
    /// Workspaces the user can use; empty while the membership is suspended.
    pub workspaces: Vec<MeWorkspace>,
}

#[derive(Serialize, ToSchema)]
pub struct Me {
    pub user: User,
    pub default_org_id: Option<Uuid>,
    pub memberships: Vec<MeMembership>,
}

#[derive(Deserialize, ToSchema)]
pub struct PatchMeRequest {
    /// 1 to 80 characters.
    pub display_name: Option<String>,
    /// Must be an organization where the caller is an active member.
    pub default_org_id: Option<Uuid>,
}

#[derive(Deserialize, ToSchema)]
pub struct DeleteMeRequest {
    /// Required for accounts that have a password.
    pub password: Option<String>,
}
