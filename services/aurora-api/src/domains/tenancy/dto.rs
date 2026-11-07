use aurora_db::repo::{orgs::OrgRow, workspaces::AccessRow, workspaces::WorkspaceRow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum OrgRole {
    Owner,
    Admin,
    Member,
}

impl OrgRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum MemberStatus {
    Active,
    Suspended,
}

impl MemberStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceRole {
    Owner,
    /// Phase 2, RFE-012: chats and decides approvals.
    Approver,
    Member,
}

impl WorkspaceRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Approver => "approver",
            Self::Member => "member",
        }
    }
}

/// Distinguishes an absent field (`None`) from an explicit `null` (`Some(None)`).
fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Serialize, ToSchema)]
pub struct OrgSummary {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
}

#[derive(Serialize, ToSchema)]
pub struct OrgSettings {
    pub members_can_create_workspaces: bool,
    pub max_members: Option<i64>,
    pub max_agents: Option<i64>,
}

impl OrgSettings {
    pub fn from_json(v: &serde_json::Value) -> Self {
        Self {
            members_can_create_workspaces: v
                .get("members_can_create_workspaces")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
            max_members: v.get("max_members").and_then(serde_json::Value::as_i64),
            max_agents: v.get("max_agents").and_then(serde_json::Value::as_i64),
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct Org {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub external_id: String,
    /// `open` | `restricted`.
    pub app_posture: String,
    pub settings: OrgSettings,
    pub my_role: OrgRole,
    pub created_at: DateTime<Utc>,
}

pub fn parse_org_role(s: &str) -> OrgRole {
    match s {
        "owner" => OrgRole::Owner,
        "admin" => OrgRole::Admin,
        _ => OrgRole::Member,
    }
}

pub fn parse_ws_role(s: &str) -> WorkspaceRole {
    match s {
        "owner" => WorkspaceRole::Owner,
        "approver" => WorkspaceRole::Approver,
        _ => WorkspaceRole::Member,
    }
}

pub fn parse_status(s: &str) -> MemberStatus {
    if s == "active" {
        MemberStatus::Active
    } else {
        MemberStatus::Suspended
    }
}

impl From<OrgRow> for Org {
    fn from(o: OrgRow) -> Self {
        Self {
            id: o.id,
            slug: o.slug,
            name: o.name,
            external_id: o.external_id,
            app_posture: o.app_posture,
            settings: OrgSettings::from_json(&o.settings),
            my_role: parse_org_role(&o.my_role),
            created_at: o.created_at,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct Workspace {
    pub id: Uuid,
    pub org_id: Uuid,
    pub slug: String,
    pub name: String,
    pub timezone: String,
    pub memory_language: String,
    /// Org owner/admin => `owner`.
    pub my_role: WorkspaceRole,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Workspace {
    pub fn new(w: WorkspaceRow, my_role: WorkspaceRole) -> Self {
        Self {
            id: w.id,
            org_id: w.org_id,
            slug: w.slug,
            name: w.name,
            timezone: w.timezone,
            memory_language: w.memory_language,
            my_role,
            created_at: w.created_at,
            updated_at: w.updated_at,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct Member {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: OrgRole,
    pub status: MemberStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
pub struct AccessBinding {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: WorkspaceRole,
    /// True for org owners/admins (no row).
    pub implicit: bool,
}

impl From<AccessRow> for AccessBinding {
    fn from(a: AccessRow) -> Self {
        Self {
            user_id: a.user_id,
            email: a.email,
            display_name: a.display_name,
            role: parse_ws_role(&a.role),
            implicit: a.implicit,
        }
    }
}

// ---- requests ----

#[derive(Deserialize, ToSchema)]
pub struct CreateOrgRequest {
    pub name: String,
    pub slug: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct CreateOrgResponse {
    pub org: Org,
    pub workspace: Workspace,
    pub default_agent: crate::domains::agents::dto::Agent,
}

#[derive(Deserialize, ToSchema)]
pub struct OrgSettingsPatch {
    pub members_can_create_workspaces: Option<bool>,
    /// Absent = unchanged, `null` = clear.
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<i64>)]
    pub max_members: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<i64>)]
    pub max_agents: Option<Option<i64>>,
}

#[derive(Deserialize, ToSchema)]
pub struct PatchOrgRequest {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub settings: Option<OrgSettingsPatch>,
}

#[derive(Deserialize, ToSchema)]
pub struct ConfirmRequest {
    pub confirm: String,
}

#[derive(Deserialize, ToSchema)]
pub struct PatchMemberRequest {
    pub role: Option<OrgRole>,
    pub status: Option<MemberStatus>,
    /// Only with `status: suspended` (RFE-021): a user id or `"workspace"`; every
    /// creator-anchored schedule and webhook of the person is reassigned in the same
    /// transaction as the suspension.
    pub reassign_to: Option<String>,
}

/// Query of the removal routes (RFE-021).
#[derive(Deserialize, utoipa::IntoParams)]
pub struct ReassignQuery {
    /// A user id or `"workspace"`: reassign the person's schedules and webhooks atomically
    /// with the removal.
    pub reassign_to: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateWorkspaceRequest {
    pub name: String,
    pub slug: Option<String>,
    pub timezone: Option<String>,
    pub memory_language: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct CreateWorkspaceResponse {
    pub workspace: Workspace,
    pub default_agent: crate::domains::agents::dto::Agent,
}

#[derive(Deserialize, ToSchema)]
pub struct PatchWorkspaceRequest {
    pub name: Option<String>,
    pub timezone: Option<String>,
    pub memory_language: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct PutAccessRequest {
    pub role: WorkspaceRole,
}

#[derive(Serialize, ToSchema)]
pub struct ResolvedWorkspace {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
}

#[derive(Serialize, ToSchema)]
pub struct ResolveResponse {
    pub org: OrgSummary,
    pub workspace: Option<ResolvedWorkspace>,
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ResolveQuery {
    pub org: String,
    pub workspace: Option<String>,
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct MembersQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    pub status: Option<MemberStatus>,
}
