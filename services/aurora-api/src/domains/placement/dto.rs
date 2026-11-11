use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

/// Where new users land (`04 §New-user placement`).
#[derive(Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum PlacementMode {
    /// New users join the default organization.
    SharedOrg,
    /// Each new user gets their own organization and owns it (the behaviour before D-140).
    PersonalOrg,
    /// No automatic placement.
    None,
}

impl PlacementMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SharedOrg => "shared_org",
            Self::PersonalOrg => "personal_org",
            Self::None => "none",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "shared_org" => Self::SharedOrg,
            "personal_org" => Self::PersonalOrg,
            _ => Self::None,
        }
    }
}

#[derive(Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum DefaultOrgRole {
    Member,
    Admin,
}

impl DefaultOrgRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::Admin => "admin",
        }
    }
}

#[derive(Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum SharedWorkspaceRole {
    Member,
    Approver,
}

impl SharedWorkspaceRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::Approver => "approver",
        }
    }
}

#[derive(Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum PersonalRole {
    Owner,
    Approver,
    Member,
}

impl PersonalRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Approver => "approver",
            Self::Member => "member",
        }
    }
}

#[derive(Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum StarterKind {
    Secret,
    Connection,
}

impl StarterKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Secret => "secret",
            Self::Connection => "connection",
        }
    }
}

#[derive(Serialize, Deserialize, ToSchema, Clone, Debug)]
pub struct SharedWorkspaceInput {
    pub workspace_id: Uuid,
    pub role: SharedWorkspaceRole,
}

#[derive(Serialize, Deserialize, ToSchema, Clone, Debug)]
pub struct StarterGrantInput {
    pub kind: StarterKind,
    pub id: Uuid,
}

#[derive(Deserialize, ToSchema, Clone, Debug)]
pub struct PersonalWorkspaceInput {
    pub enabled: bool,
    /// Default `owner`.
    #[serde(default)]
    pub role: Option<PersonalRole>,
    /// `{variable}` text with `|` between fallbacks; default `{sub}|{email_local}`.
    #[serde(default)]
    pub name_template: Option<String>,
}

/// `PUT /v1/admin/placement` (C-521).
#[derive(Deserialize, ToSchema, Debug)]
#[serde(deny_unknown_fields)]
pub struct PutPlacement {
    pub mode: PlacementMode,
    /// `shared_org`: an existing organization (exactly one of this and `create_default_org`).
    #[serde(default)]
    pub default_org_id: Option<Uuid>,
    /// `shared_org`: create an organization with this name; the caller becomes its owner.
    #[serde(default)]
    pub create_default_org: Option<String>,
    /// Default `member`.
    #[serde(default)]
    pub default_org_role: Option<DefaultOrgRole>,
    #[serde(default)]
    pub shared_workspaces: Vec<SharedWorkspaceInput>,
    #[serde(default)]
    pub personal_workspace: Option<PersonalWorkspaceInput>,
    #[serde(default)]
    pub starter_grants: Vec<StarterGrantInput>,
}

#[derive(Serialize, ToSchema)]
pub struct OrgRef {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
}

#[derive(Serialize, ToSchema)]
pub struct SharedWorkspaceView {
    pub workspace_id: Uuid,
    pub slug: String,
    pub name: String,
    pub role: SharedWorkspaceRole,
}

#[derive(Serialize, ToSchema)]
pub struct PersonalWorkspaceView {
    pub enabled: bool,
    pub role: PersonalRole,
    pub name_template: String,
}

#[derive(Serialize, ToSchema)]
pub struct StarterGrantView {
    pub kind: StarterKind,
    pub id: Uuid,
    /// The credential's name (secret name or connection label).
    pub name: String,
    /// `llm` | `http` for secrets; the app id for connections.
    pub detail: String,
}

#[derive(Serialize, ToSchema)]
pub struct PersonalOrgView {
    pub org_id: Uuid,
    pub slug: String,
    pub name: String,
    pub owner_email: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
pub struct PersonalOrgs {
    /// All personal orgs from before, even when `items` is capped.
    pub count: i64,
    pub items: Vec<PersonalOrgView>,
}

/// `GET /v1/admin/placement` and the `PUT` response (C-520).
#[derive(Serialize, ToSchema)]
pub struct PlacementView {
    /// `false` until an instance admin saves: the instance then behaves as before D-140.
    pub saved: bool,
    /// The configured `auth.signup_mode` (read-only here).
    pub signup_mode: String,
    pub mode: PlacementMode,
    pub default_org: Option<OrgRef>,
    pub default_org_role: DefaultOrgRole,
    pub shared_workspaces: Vec<SharedWorkspaceView>,
    pub personal_workspace: PersonalWorkspaceView,
    pub starter_grants: Vec<StarterGrantView>,
    pub personal_orgs: PersonalOrgs,
    /// `placement_unsaved` | `per_user_orgs_active` | `default_org_missing`.
    pub warnings: Vec<String>,
    pub updated_at: Option<DateTime<Utc>>,
    pub updated_by: Option<Uuid>,
}

#[derive(Deserialize, IntoParams)]
pub struct ChoicesQuery {
    /// The candidate default organization.
    pub org_id: Uuid,
}

#[derive(Serialize, ToSchema)]
pub struct ChoiceWorkspace {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
}

#[derive(Serialize, ToSchema)]
pub struct ChoiceSecret {
    pub id: Uuid,
    pub name: String,
    /// `llm` | `generic` …
    pub kind: String,
}

#[derive(Serialize, ToSchema)]
pub struct ChoiceConnection {
    pub id: Uuid,
    pub label: String,
    pub app_id: String,
}

/// `GET /v1/admin/placement/choices` (C-528): what the form may offer for an organization.
#[derive(Serialize, ToSchema)]
pub struct PlacementChoices {
    /// Ordinary workspaces (never personal ones).
    pub workspaces: Vec<ChoiceWorkspace>,
    /// Active organization-level secrets, LLM keys included.
    pub secrets: Vec<ChoiceSecret>,
    /// Organization-level connections.
    pub connections: Vec<ChoiceConnection>,
}

#[derive(Deserialize, IntoParams)]
pub struct PreviewQuery {
    /// The template to render (default: the saved one).
    pub template: Option<String>,
    /// A typed sample ("bobj", "Ada Lovelace") standing for every simple variable; without it
    /// the caller's own profile is used. The sample never stands for `{sub}` or `{claim:…}`.
    pub sample: Option<String>,
}

/// `GET /v1/admin/placement/preview` (C-522).
#[derive(Serialize, ToSchema)]
pub struct PlacementPreview {
    /// The workspace name the first sign-in would create.
    pub name: String,
    /// Its slug before any numbered suffix (`ada-lovelace`).
    pub slug: String,
    /// Index of the fallback alternative used; `null` = the `user-<short id>` last resort.
    pub used: Option<usize>,
    /// Variables with no value here (and so skipped or empty).
    pub unresolved: Vec<String>,
}
