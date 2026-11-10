//! Directory groups DTOs (phase 4, P4-GRP-1, D-130, D-156; C-460..C-469).
use crate::domains::tenancy::dto::WorkspaceRole;
use aurora_db::repo::groups::{
    BindingRow, DerivedMember as DerivedRow, GroupRow, ImpactWorkspace, MemberRow,
    WorkspaceGroupBinding,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

/// `manual` (dashboard/API), `scim` (the IdP via SCIM) or `idp` (membership synced from a
/// sign-in claim, D-156).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum GroupSource {
    Manual,
    Scim,
    Idp,
}

impl GroupSource {
    pub fn parse(s: &str) -> Self {
        match s {
            "scim" => Self::Scim,
            "idp" => Self::Idp,
            _ => Self::Manual,
        }
    }
}

/// `admin` or `member`; `owner` is never a legal mapping (SEC-71).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum GroupOrgRole {
    Admin,
    Member,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Group {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub source: GroupSource,
    /// (D-156) For `source = idp`: the provider id and claim value the group mirrors.
    pub idp_provider: Option<String>,
    pub idp_value: Option<String>,
    pub org_role: Option<GroupOrgRole>,
    pub member_count: i64,
    pub workspace_count: i64,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
}

impl From<GroupRow> for Group {
    fn from(g: GroupRow) -> Self {
        Self {
            id: g.id,
            org_id: g.org_id,
            name: g.name,
            description: g.description,
            source: GroupSource::parse(&g.source),
            idp_provider: g.idp_provider,
            idp_value: g.idp_value,
            org_role: match g.org_role.as_deref() {
                Some("admin") => Some(GroupOrgRole::Admin),
                Some("member") => Some(GroupOrgRole::Member),
                _ => None,
            },
            member_count: g.member_count,
            workspace_count: g.workspace_count,
            revision: g.revision,
            created_at: g.created_at,
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

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateGroupRequest {
    pub name: String,
    pub description: Option<String>,
    /// `admin` | `member`; `owner` is refused (422 `group_org_role_invalid`).
    pub org_role: Option<String>,
    /// `manual` (default) or `idp` (D-156); `scim` groups are created by the IdP.
    pub source: Option<String>,
    /// (D-156) Required for `source = idp`: a configured OIDC provider id with a `group_claim`.
    pub idp_provider: Option<String>,
    /// (D-156) Required for `source = idp`: the exact claim value (≤ 256 chars).
    pub idp_value: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PatchGroupRequest {
    pub name: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<String>)]
    pub description: Option<Option<String>>,
    /// `admin` | `member` | `null` (clear); `owner` is refused (422 `group_org_role_invalid`).
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<String>)]
    pub org_role: Option<Option<String>>,
    /// The revision the caller last saw (409 `stale_revision`); the `If-Match` header works too.
    pub revision: Option<i64>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct GroupListQuery {
    /// Case-insensitive name substring.
    pub q: Option<String>,
    /// `manual` | `scim` | `idp`.
    pub source: Option<String>,
    /// Only groups that contain this user.
    pub member: Option<Uuid>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UserRef {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GroupMember {
    pub user: UserRef,
    /// The user's org membership status: a `suspended` member stays listed and derives nothing.
    pub status: String,
    pub source: GroupSource,
    /// (D-156) The sign-in that last confirmed an `idp` membership.
    pub synced_at: Option<DateTime<Utc>>,
    pub added_at: DateTime<Utc>,
}

impl From<MemberRow> for GroupMember {
    fn from(m: MemberRow) -> Self {
        Self {
            user: UserRef {
                id: m.user_id,
                email: m.email,
                display_name: m.display_name,
            },
            status: m.status,
            source: GroupSource::parse(&m.source),
            synced_at: m.synced_at,
            added_at: m.created_at,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct MembersBatchRequest {
    #[serde(default)]
    pub add: Vec<Uuid>,
    #[serde(default)]
    pub remove: Vec<Uuid>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MembersBatchResult {
    /// Members actually added (already-present users are skipped).
    pub added: Vec<Uuid>,
    /// Members actually removed (absent users are skipped).
    pub removed: Vec<Uuid>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceRef {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GroupBinding {
    pub workspace: WorkspaceRef,
    pub role: WorkspaceRole,
    pub created_at: DateTime<Utc>,
}

impl From<BindingRow> for GroupBinding {
    fn from(b: BindingRow) -> Self {
        Self {
            workspace: WorkspaceRef {
                id: b.workspace_id,
                name: b.workspace_name,
                slug: b.workspace_slug,
            },
            role: crate::domains::tenancy::dto::parse_ws_role(&b.role),
            created_at: b.created_at,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PutGroupAccessRequest {
    pub role: WorkspaceRole,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GroupAccessBinding {
    pub group: GroupSummary,
    pub role: WorkspaceRole,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GroupSummary {
    pub id: Uuid,
    pub name: String,
    pub source: GroupSource,
    pub member_count: i64,
}

impl From<WorkspaceGroupBinding> for GroupAccessBinding {
    fn from(b: WorkspaceGroupBinding) -> Self {
        Self {
            group: GroupSummary {
                id: b.group_id,
                name: b.name,
                source: GroupSource::parse(&b.source),
                member_count: b.member_count,
            },
            role: crate::domains::tenancy::dto::parse_ws_role(&b.role),
        }
    }
}

/// Why a person has access: informational only (`04 §Groups`).
#[derive(Debug, Serialize, ToSchema)]
pub struct AccessSourceView {
    /// `direct` | `idp_claim` | `group` | `org_role` | `org_role_via_group`.
    pub kind: String,
    /// The group, for `group` and `org_role_via_group`.
    pub group: Option<GroupName>,
    /// The `idp:<provider>:<value>` source, for `idp_claim`.
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct GroupName {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DerivedMemberView {
    pub user: UserRef,
    pub role: WorkspaceRole,
    pub sources: Vec<AccessSourceView>,
}

impl DerivedMemberView {
    pub fn from_row(r: DerivedRow, names: &std::collections::HashMap<Uuid, String>) -> Self {
        Self {
            user: UserRef {
                id: r.user_id,
                email: r.email,
                display_name: r.display_name,
            },
            role: crate::domains::tenancy::dto::parse_ws_role(&r.role),
            sources: r
                .sources
                .into_iter()
                .map(|s| AccessSourceView {
                    kind: s.kind.as_str().to_owned(),
                    group: s.group.map(|id| GroupName {
                        id,
                        name: names.get(&id).cloned().unwrap_or_default(),
                    }),
                    value: s.value,
                })
                .collect(),
        }
    }
}

/// Query of `GET /workspaces/{ws}/access`.
#[derive(Debug, Deserialize, IntoParams)]
pub struct AccessQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    /// `groups` (phase 4, P4-GRP-1): add `group_bindings` and `derived_members` (`can_own`).
    pub expand: Option<String>,
    /// Cursor of the `derived_members` page.
    pub derived_cursor: Option<String>,
}

/// `GET /workspaces/{ws}/access`: the direct bindings (unchanged) and, with `expand=groups`,
/// the groups bound to the workspace and everyone who has access through a group or an org role.
#[derive(Serialize, ToSchema)]
pub struct AccessList {
    pub items: Vec<crate::domains::tenancy::dto::AccessBinding>,
    pub next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_bindings: Option<Vec<GroupAccessBinding>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub derived_members: Option<crate::http::pagination::Page<DerivedMemberView>>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ImpactQuery {
    /// Show what removing only this member would take away.
    pub remove_member: Option<Uuid>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ImpactUser {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ImpactWorkspaceView {
    pub workspace: WorkspaceRef,
    /// At most `GROUP_IMPACT_LIST_MAX` (200); `total` is always given.
    pub users_losing_access: Vec<ImpactUser>,
    pub total: i64,
    pub schedules_disabled: i64,
    pub webhooks_disabled: i64,
    pub api_keys: i64,
}

impl From<ImpactWorkspace> for ImpactWorkspaceView {
    fn from(i: ImpactWorkspace) -> Self {
        Self {
            workspace: WorkspaceRef {
                id: i.workspace_id,
                name: i.workspace_name,
                slug: i.workspace_slug,
            },
            users_losing_access: i
                .users_losing_access
                .into_iter()
                .map(|(id, email, display_name)| ImpactUser {
                    id,
                    email,
                    display_name,
                })
                .collect(),
            total: i.total,
            schedules_disabled: i.schedules_disabled,
            webhooks_disabled: i.webhooks_disabled,
            api_keys: i.api_keys,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GroupImpact {
    pub workspaces: Vec<ImpactWorkspaceView>,
}
