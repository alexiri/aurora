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
    /// (D-175, FIX-082) Prepared by SCIM for an existing account; confers nothing until that
    /// person's next verified sign-in. Read-only: never accepted as a `PATCH` value.
    Pending,
}

impl MemberStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
            Self::Pending => "pending",
        }
    }
}

/// (P5-APIGAP-4, C-1051) A workspace binding's status: `suspended` grants nothing in that
/// workspace (SEC-138).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AccessStatus {
    Active,
    Suspended,
}

impl AccessStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
        }
    }
    fn parse(s: &str) -> Self {
        // Fail closed: anything but `active` reads as suspended (the CHECK allows only these two).
        if s == "active" {
            Self::Active
        } else {
            Self::Suspended
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
    /// (phase 4, P4-PRV-1, C-549) Port previews for this organization (default `false`).
    pub previews_enabled: bool,
    /// (phase 5, P5-CALINV, D-274, C-844) Calendar-invitation tools for the organization's agents
    /// (default `true`); stored as `settings.calendar_invitations.enabled`.
    pub calendar_invitations_enabled: bool,
    /// (phase 5, P5-WEBHOOK-2, D-320, C-912) Outgoing webhooks for the organization (default
    /// `true`); stored as `settings.outgoing_webhooks.enabled`.
    pub outgoing_webhooks_enabled: bool,
    /// The org's own send ceilings (`null` = the instance's), stored as
    /// `settings.outgoing_webhooks.per_minute_max` / `.per_day_max`.
    pub outgoing_webhooks_per_minute_max: Option<i64>,
    pub outgoing_webhooks_per_day_max: Option<i64>,
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
            previews_enabled: v
                .get("previews_enabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            calendar_invitations_enabled: Self::calendar_invitations_enabled(v),
            outgoing_webhooks_enabled: Self::outgoing_webhooks_enabled(v),
            outgoing_webhooks_per_minute_max: v
                .pointer("/outgoing_webhooks/per_minute_max")
                .and_then(serde_json::Value::as_i64),
            outgoing_webhooks_per_day_max: v
                .pointer("/outgoing_webhooks/per_day_max")
                .and_then(serde_json::Value::as_i64),
        }
    }

    /// The org switch of outgoing webhooks (default on).
    pub fn outgoing_webhooks_enabled(v: &serde_json::Value) -> bool {
        v.pointer("/outgoing_webhooks/enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true)
    }

    /// The org switch of the calendar-invitation tools (default on).
    pub fn calendar_invitations_enabled(v: &serde_json::Value) -> bool {
        v.pointer("/calendar_invitations/enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true)
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
    /// (phase 2, RFE-006) The organization's freeze; `null` when not frozen.
    pub frozen: Option<crate::domains::freeze::dto::Frozen>,
    /// (P5-UX-2, C-981) Who to ask: owners and admins, up to three names each. Present on
    /// `GET /v1/orgs/{org}` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contacts: Option<crate::http::who_can::Contacts>,
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
    match s {
        "active" => MemberStatus::Active,
        "pending" => MemberStatus::Pending,
        _ => MemberStatus::Suspended,
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
            frozen: None,
            contacts: None,
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
    /// (phase 2, RFE-025) The workspace's own BYO posture (`null` = inherit).
    pub byo_posture: Option<String>,
    /// (phase 2, RFE-025) The effective BYO posture and the layer that decided it; `null` in
    /// list and create responses.
    pub effective_byo_posture: Option<crate::domains::agents::byo::ByoPostureView>,
    /// (FIX-234, C-1160) The caller's own workspace role; `null` when they hold none — an
    /// organization role grants no workspace right (D-477). An administrator without own
    /// access has `administers: true` instead.
    pub my_role: Option<WorkspaceRole>,
    /// (FIX-234, C-1160) The caller manages this workspace through their organization role
    /// (own-right `owner` there or an active org owner/admin) but may hold no own access.
    pub administers: bool,
    /// (FIX-236, C-1165) True while the caller holds an active emergency read access here.
    pub emergency_reader: bool,
    /// (FIX-236, C-1165, review HIGH-3) The caller's OWN active emergency grant here — the id
    /// their reader banner's "End now" ends, never another owner's newest grant; `null` when
    /// they hold no active grant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emergency_reader_id: Option<Uuid>,
    /// (FIX-236, verify LOW-2) The caller's OWN active grant's expiry — what their reader
    /// banner counts down (`audience.emergency` may be another owner's newest grant); `null`
    /// when they hold no active grant. Never set for an API key: keys cannot use a grant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emergency_reader_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// (FIX-234, C-1162) Who the workspace's people are; present on `GET /v1/workspaces/{ws}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audience: Option<WorkspaceAudience>,
    /// (phase 2, RFE-006) The freeze that stops this workspace's agents (its own or its
    /// organization's; the broadest wins), `level` = where it is set; `null` when not frozen.
    pub frozen: Option<crate::domains::freeze::dto::Frozen>,
    /// (P5-UX-2, C-981) The workspace owners to ask (up to three names); present on
    /// `GET /v1/workspaces/{ws}` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contacts: Option<crate::http::who_can::Contacts>,
    /// (P5-UX-11, C-947) Present on a personal workspace in `GET /v1/orgs/{org}/workspaces`: the
    /// person it was made for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub personal_of: Option<PersonalOf>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Workspace {
    pub fn new(w: WorkspaceRow, my_role: Option<WorkspaceRole>, administers: bool) -> Self {
        Self {
            id: w.id,
            org_id: w.org_id,
            slug: w.slug,
            name: w.name,
            timezone: w.timezone,
            memory_language: w.memory_language,
            byo_posture: w.byo_posture,
            effective_byo_posture: None,
            my_role,
            administers,
            emergency_reader: false,
            emergency_reader_id: None,
            emergency_reader_expires_at: None,
            audience: None,
            frozen: None,
            contacts: None,
            personal_of: None,
            created_at: w.created_at,
            updated_at: w.updated_at,
        }
    }
}

/// (FIX-234, C-1162) `Workspace.audience`: the workspace's people in their own right and its
/// own-right owners. Names only; never content.
#[derive(Serialize, ToSchema)]
pub struct WorkspaceAudience {
    /// Distinct active people with access in their own right.
    pub people: i64,
    /// The own-right owners: at most 5, the caller first if an owner, then by display name.
    pub owners: Vec<WorkspaceAudienceOwner>,
    /// How many own-right owners the workspace has.
    pub owners_total: i64,
    /// (FIX-236, C-1165) The newest **active** emergency access when any is active (review
    /// HIGH-1: an owner's open access is never hidden behind another owner's newer, ended one),
    /// else the newest that ended within the last 7 days — the workspace's banner for its
    /// people; `null` when there is none to show.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emergency: Option<WorkspaceAudienceEmergency>,
    /// (FIX-236, review HIGH-1) How many **other** emergency grants are active right now besides
    /// `emergency` — two simultaneous openers are both announced.
    pub emergency_active_more: i64,
}

/// (FIX-236, C-1165) `audience.emergency`: who opened emergency read access, why and until
/// when. Metadata only; the reason is the opener's own sanitized words.
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceAudienceEmergency {
    pub id: Uuid,
    pub by: WorkspaceAudienceOwner,
    pub reason: String,
    pub opened_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub active: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceAudienceOwner {
    pub id: Uuid,
    pub display_name: String,
}

/// (P4-GRP-2) The SCIM projection of the Members page: `directory` is present only with a
/// `scim_users` link (`state` + the IdP's `externalId`).
#[derive(Serialize, ToSchema)]
pub struct MemberDirectory {
    pub state: String,
    pub external_id: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct Member {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: OrgRole,
    pub status: MemberStatus,
    pub created_at: DateTime<Utc>,
    /// (phase 4, P4-IDP-1, C-514) The IdP role value that gives this role ("From IdP role …"
    /// badge); `null` for a manual role. A role with a value cannot be edited or removed (409
    /// `managed_by_idp`).
    pub idp_value: Option<String>,
    /// (phase 4, P4-GRP-2) The SCIM directory link; absent when the member has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory: Option<MemberDirectory>,
}

#[derive(Serialize, ToSchema)]
pub struct AccessBinding {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: WorkspaceRole,
    /// Always `false` since FIX-234 (D-477): an organization role grants no workspace right, so
    /// no implicit rows are returned. Kept one release for older dashboards (FIX-235 stops
    /// reading it).
    pub implicit: bool,
    /// (phase 4, P4-IDP-1, C-514) The IdP role value behind this binding; `null` when manual.
    pub idp_value: Option<String>,
    /// (phase 5, P5-APIGAP-4, C-1050) When the direct binding was created.
    pub joined_at: Option<DateTime<Utc>>,
    /// (phase 5, P5-APIGAP-4, C-1051) `suspended`: the person has no access to this workspace
    /// (direct or through a group) until restored.
    pub status: AccessStatus,
    /// (P5-REFINE-6, C-1086) The person has signed in at least once ("Not signed in yet").
    pub signed_in: bool,
}

impl From<AccessRow> for AccessBinding {
    fn from(a: AccessRow) -> Self {
        Self {
            user_id: a.user_id,
            email: a.email,
            display_name: a.display_name,
            role: parse_ws_role(&a.role),
            implicit: a.implicit,
            idp_value: a.idp_value,
            joined_at: a.joined_at,
            status: AccessStatus::parse(&a.status),
            signed_in: a.signed_in,
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
    /// (phase 4, P4-PRV-1, C-549) Port previews for the organization; audited
    /// `org.previews.update` (security-critical).
    pub previews_enabled: Option<bool>,
    /// (phase 5, P5-CALINV, C-844) The calendar-invitation tools for the organization's agents.
    pub calendar_invitations_enabled: Option<bool>,
    /// (phase 5, P5-WEBHOOK-2, C-912) The outgoing-webhook switch; audited with `org.update`.
    pub outgoing_webhooks_enabled: Option<bool>,
    /// The org's send ceilings. Absent = unchanged, `null` = the instance's; above the instance's →
    /// 422 `above_instance_ceiling`.
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<i64>)]
    pub outgoing_webhooks_per_minute_max: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<i64>)]
    pub outgoing_webhooks_per_day_max: Option<Option<i64>>,
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
    /// (phase 2, RFE-025) `enabled` | `owners_only` | `disabled`; absent = unchanged, `null` =
    /// inherit. Tighten-only.
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<String>)]
    pub byo_posture: Option<Option<String>>,
}

#[derive(Deserialize, ToSchema)]
pub struct PutAccessRequest {
    pub role: WorkspaceRole,
}

/// (phase 5, P5-APIGAP-4, C-1051) `PATCH /workspaces/{ws}/access/{user}`: suspend or restore.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchAccessRequest {
    pub status: AccessStatus,
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

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct WorkspaceOverridesQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    /// Every workspace, not only those that differ from the organization.
    pub all: Option<bool>,
}

#[derive(Serialize, ToSchema)]
pub struct OverrideWorkspace {
    pub id: Uuid,
    pub name: String,
    /// Present on a personal workspace: the person it was made for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub personal_of: Option<PersonalOf>,
}

#[derive(Serialize, ToSchema)]
pub struct PersonalOf {
    pub id: Uuid,
    pub name: String,
}

/// (C-964) What one workspace's published policy sets below its organization.
#[derive(Serialize, ToSchema)]
pub struct WorkspaceOverride {
    pub workspace: OverrideWorkspace,
    /// `block` when requests without an account are blocked unless allow-listed.
    pub uncredentialed: String,
    /// `allowlist_only` or `disabled` when browser connections are tightened.
    pub open_lane: String,
    /// The workspace's default rule: `none`, `allow` or `block`.
    pub default_rule: String,
    pub rule_count: i64,
}
