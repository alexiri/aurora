use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AppsQuery {
    /// Workspace whose availability and connection counts apply.
    pub workspace_id: Option<uuid::Uuid>,
}

/// (P5-UX-2, C-931, C-955) The level a setup status or connect plan is for.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SetupQuery {
    /// A workspace view (needs workspace owner).
    pub workspace_id: Option<uuid::Uuid>,
    /// An organization view when `workspace_id` is absent (needs organization owner or admin).
    pub org_id: Option<uuid::Uuid>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct IconQuery {
    /// `light` (default) or `dark`.
    pub variant: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AppSummary {
    pub app_id: String,
    pub package_id: String,
    pub package_version: String,
    pub name: String,
    pub description: String,
    pub category: Option<String>,
    pub available: bool,
    pub connection_count: i64,
    pub has_dark_icon: bool,
    /// (phase 4, P4-GRANTS, D-123, C-414) the manifest's `grants` flags, so the dashboard can
    /// show "Custom access only" and drop the full-access choice.
    pub app_restrictions: crate::domains::grants::dto::AppRestrictions,
    /// (phase 4, P4-GSL, D-149) the app declares a `library` block: it can serve a git skill
    /// library (the dashboard offers its org-scoped connections when connecting a repository).
    pub serves_library: bool,
    /// (phase 5, FIX-135, D-242, C-744) The product this app is a deployment of; `null` for an
    /// app that is its own product.
    pub family: Option<AppFamily>,
    /// (FIX-154, D-306(c), C-895) Nothing to sign in to or enter (no credentials, token,
    /// instances or database; every connect method is a `fields` method with no fields): an agent
    /// is given access in one step. Derived from the installed manifest, not stored.
    pub credential_free: bool,
    /// (P5-POLISH-3, D-387, C-1029, C-1031) The manifest's `about.summary`: one plain sentence cards show
    /// whole; `null` when the package has none (clients then clamp `description`, never cutting mid-word).
    pub summary: Option<String>,
    /// (P5-POLISH-3, D-388, C-1029) Whether the connect journey can start now for the caller; only in the
    /// catalogue list with a `workspace_id` (`null` otherwise).
    pub readiness: Option<super::setup::AppReadiness>,
    /// (C-1029) The workspace's accounts of this app the caller may give to an agent; only in the catalogue
    /// list with a `workspace_id`.
    pub accounts: Vec<AppAccountRef>,
}

/// (C-1029) An account of an app, with the agents that already use it.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AppAccountRef {
    pub id: Uuid,
    pub name: String,
    pub agent_ids: Vec<Uuid>,
}

/// `about.family` of an app (C-744): the catalogue and every picker show one product, named
/// "<name> · <variant_label>" wherever an app is named.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AppFamily {
    pub id: String,
    /// The product name, the same on every member.
    pub name: String,
    /// This app's deployment (its host, "Self-hosted", "Data Center"), unique within the family.
    pub variant_label: String,
    /// The app is served by registered self-hosted servers (it declares `instances`).
    pub instance_bound: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AppList {
    pub items: Vec<AppSummary>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ConnectNote {
    pub text: String,
    pub url: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
    pub warning: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ShowWhen {
    pub field: String,
    pub equals: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FormField {
    pub name: String,
    pub label: String,
    /// `text` | `secret` | `select` | `file` | `json_file`.
    #[serde(rename = "type")]
    pub kind: String,
    /// `credentials` | `metadata` | `tenant_host`.
    pub store: String,
    pub required: bool,
    pub placeholder: Option<String>,
    pub help_url: Option<String>,
    pub options: Option<Vec<SelectOption>>,
    pub show_when: Option<ShowWhen>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ScopeInfo {
    pub scope: String,
    pub name: String,
    /// `read` | `write`.
    pub access: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AccessLevel {
    pub id: String,
    pub label: String,
}

/// (P5-POLISH-3, D-387, C-1031) The permissions to tick on the app's own token screen for one access level, in
/// the app's own words ("Contents · Read"). For a token method the levels are the journey's `read` and `write`.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct LevelHints {
    /// The access level's id (`read` | `write` for a token method).
    pub level: String,
    pub label: String,
    pub hints: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ConnectMethod {
    pub id: String,
    /// `oauth2` | `fields` | `hook`.
    pub kind: String,
    /// (FIX-123, D-214) Short name for the connect window's "How do you want to connect?"
    /// chooser; `null` = the window falls back to the kind.
    pub label: Option<String>,
    /// One or two sentences: what the method is and when to use it.
    pub description: Option<String>,
    /// The chooser preselects this method and badges it "Recommended" (at most one per app).
    pub recommended: bool,
    pub fields: Vec<FormField>,
    pub scopes: Vec<ScopeInfo>,
    pub default_scopes: Vec<String>,
    pub access_levels: Vec<AccessLevel>,
    pub client_key: Option<String>,
    /// `query` | `fragment` (OAuth only).
    pub callback: Option<String>,
    /// (FIX-136) For a `fields` method: the read scopes whose probe prefills the declaration
    /// ("Which scopes did you give this token?"); the others are asked without a prefill.
    pub probe_scopes: Vec<String>,
    /// (P5-UX-2, C-956) Who the method is for; an `admin` method is never suggested.
    pub audience: super::setup::MethodAudience,
    /// (P5-UX-2, C-956) The scopes of "Read only" and "Read and change", already defaulted.
    pub scope_presets: super::setup::MethodScopePresets,
    /// (P5-POLISH-3, D-387, C-1031) The permissions to tick on the app's token screen, per access level.
    pub permission_hints: Vec<LevelHints>,
    /// (P5-UX-13, C-942) Values the person copies into the other service, shown with Copy above
    /// the method's fields: `org_external_id` (the AWS role's trust policy).
    pub show_context: Vec<String>,
    /// (P5-UX-14, C-945) Whether connecting with this method already probes the upstream. The
    /// journey tests a new account once afterwards only when it does not.
    pub has_probe: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Tool {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// `read` | `write`.
    pub group: String,
    pub wildcard: bool,
    pub host: String,
    pub path: String,
    pub methods: Vec<String>,
    /// (FIX-123, D-214) Connect method ids the tool works with; empty = every method.
    pub connect_methods: Vec<String>,
    /// (FIX-136, D-241) Scope ids (any of) the tool needs; empty = no requirement.
    pub scopes: Vec<String>,
    /// `query` | `mutation`.
    pub graphql: Option<String>,
    /// `push` | `fetch`.
    pub git: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HostInfo {
    pub host: String,
    pub path_prefix: Option<String>,
    /// `active` | `lost`.
    pub claim_status: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CredentialInfo {
    pub name: String,
    pub optional: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OauthClientField {
    pub name: String,
    pub label: String,
    pub secret: bool,
}

/// An OAuth client definition the app's connect methods use (bring-your-own-client forms).
#[derive(Debug, Serialize, ToSchema)]
pub struct AppOauthClient {
    pub client_key: String,
    pub fields: Vec<OauthClientField>,
    pub setup_note: Option<String>,
    pub redirect_uri: String,
    /// (P5-UX-2, C-946) The connect methods that use this client: forms and lists say
    /// "<app> — for <method label>", never a bare `client_key`.
    pub used_by_methods: Vec<super::setup::MethodRef>,
}

/// An instance-bound app's `instances` declaration (C-245, D-74).
#[derive(Debug, Serialize, ToSchema)]
pub struct AppInstances {
    /// Shown in the registration form and the connect picker.
    pub label: String,
    /// The SaaS host (a normal static claim); `null` when the app has no hosted default.
    pub default_host: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AppDetail {
    pub app_id: String,
    pub package_id: String,
    pub package_version: String,
    pub name: String,
    pub description: String,
    pub category: Option<String>,
    pub available: bool,
    pub connection_count: i64,
    pub has_dark_icon: bool,
    /// (P5-POLISH-3, C-1031) As on `AppSummary`.
    pub summary: Option<String>,
    /// (P5-POLISH-3, D-387, C-1031) A question a person could send to try the app; the journey's Done
    /// step opens it as a chat draft.
    pub first_prompt: Option<String>,
    /// (phase 4, P4-GRANTS, D-123, C-414) the manifest's `grants` flags.
    pub app_restrictions: crate::domains::grants::dto::AppRestrictions,
    /// (phase 4, P4-GSL) as on `AppSummary`.
    pub serves_library: bool,
    /// (phase 5, FIX-135, C-744) as on `AppSummary`.
    pub family: Option<AppFamily>,
    /// (FIX-154, C-895) as on `AppSummary`.
    pub credential_free: bool,
    pub api_docs: Option<String>,
    pub connect_note: Option<ConnectNote>,
    pub label_hint: Option<String>,
    pub connect: Vec<ConnectMethod>,
    pub tools: Vec<Tool>,
    pub hosts: Vec<HostInfo>,
    pub agent_instructions: Option<String>,
    pub credentials: Vec<CredentialInfo>,
    pub metadata: Vec<String>,
    pub oauth_clients: Vec<AppOauthClient>,
    /// `null` when the app is not instance-bound.
    pub instances: Option<AppInstances>,
}
