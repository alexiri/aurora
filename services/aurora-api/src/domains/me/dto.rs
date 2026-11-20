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
    /// (phase 4, P4-ONB-2, C-531) This is the caller's own personal workspace (D-140): the
    /// Welcome page opens there.
    pub personal: bool,
}

#[derive(Serialize, ToSchema)]
pub struct MeMembership {
    pub org: OrgSummary,
    pub role: OrgRole,
    pub status: MemberStatus,
    /// (phase 4, P4-GRP-1) The effective org role: the strongest of `role` and the `org_role` of
    /// the caller's groups, only while the membership is active (`role` stays the direct role);
    /// `null` for a suspended membership (C-465).
    pub effective_role: Option<OrgRole>,
    /// Where the effective role comes from: `direct`, or `group` with the group.
    pub sources: Vec<MeRoleSource>,
    /// Workspaces the user can use; empty while the membership is suspended.
    pub workspaces: Vec<MeWorkspace>,
}

#[derive(Serialize, ToSchema)]
pub struct MeRoleSource {
    /// `direct` | `group`.
    pub kind: String,
    pub group: Option<MeGroupRef>,
}

#[derive(Serialize, ToSchema)]
pub struct MeGroupRef {
    pub id: Uuid,
    pub name: String,
}

/// A group the caller belongs to (phase 4, P4-GRP-1).
#[derive(Serialize, ToSchema)]
pub struct MeGroup {
    pub id: Uuid,
    pub name: String,
    pub org_id: Uuid,
}

#[derive(Serialize, ToSchema)]
pub struct Me {
    pub user: User,
    pub default_org_id: Option<Uuid>,
    /// (phase 2, RFE-011a) E-mail me approvals nobody is watching (default `true`).
    pub approval_email: bool,
    /// (FIX-167 Part B, C-1024) E-mail me when something new needs an instance admin's decision
    /// (default `true`; the dashboard shows it to instance admins only).
    pub admin_attention_email: bool,
    /// (P2-FU-1, C-195) The caller's own client address as this API resolves it: the socket peer,
    /// or the rightmost untrusted `X-Forwarded-For` hop when the peer is in
    /// `server.trusted_proxies`. A hint for allow-lists; the gateway may see another address
    /// (NAT, load balancer, PROXY protocol). `null` when unknown. Set on `GET`/`PATCH /v1/me`.
    pub client_address: Option<String>,
    pub memberships: Vec<MeMembership>,
    /// (phase 4, P4-GRP-1) The caller's groups (active memberships only).
    pub groups: Vec<MeGroup>,
    /// (phase 4, P4-IDP-1, C-513) "Roles from your sign-in": the role values of the last sign-in
    /// and what each granted; `null` when no sign-in read a role claim.
    pub idp_roles: Option<crate::domains::admin::dto::IdpRoles>,
    /// (P5-PERSON, C-1054) The person's own IANA time zone; `null` = the workspace's is used.
    pub timezone: Option<String>,
    /// (P5-REFINE-5, C-1064, D-407) Where `user.display_name` comes from: `sign_in` (the token's
    /// `name`, refreshed at every sign-in) or `you` (the person, or their directory, chose it).
    pub display_name_source: String,
}

fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Deserialize, ToSchema)]
pub struct PatchMeRequest {
    /// 1 to 80 characters.
    pub display_name: Option<String>,
    /// Must be an organization where the caller is an active member.
    pub default_org_id: Option<Uuid>,
    /// (phase 2, RFE-011a) Opt in or out of approval e-mails.
    #[serde(default)]
    pub approval_email: Option<bool>,
    /// (FIX-167 Part B, C-1024) Opt in or out of the e-mail about new administration decisions.
    /// Stored for anyone, used only for an instance admin.
    #[serde(default)]
    pub admin_attention_email: Option<bool>,
    /// (P5-PERSON, C-1054) An IANA time zone name; `null` clears it; absent leaves it. Not a
    /// known name → 422 `value_invalid`.
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<String>)]
    pub timezone: Option<Option<String>>,
}
