use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AppsQuery {
    /// Workspace whose availability and connection counts apply.
    pub workspace_id: Option<uuid::Uuid>,
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

#[derive(Debug, Serialize, ToSchema)]
pub struct ConnectMethod {
    pub id: String,
    /// `oauth2` | `fields` | `hook`.
    pub kind: String,
    pub fields: Vec<FormField>,
    pub scopes: Vec<ScopeInfo>,
    pub default_scopes: Vec<String>,
    pub access_levels: Vec<AccessLevel>,
    pub client_key: Option<String>,
    /// `query` | `fragment` (OAuth only).
    pub callback: Option<String>,
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
    pub api_docs: Option<String>,
    pub connect_note: Option<ConnectNote>,
    pub label_hint: Option<String>,
    pub connect: Vec<ConnectMethod>,
    pub tools: Vec<Tool>,
    pub hosts: Vec<HostInfo>,
    pub agent_instructions: Option<String>,
    pub credentials: Vec<CredentialInfo>,
    pub metadata: Vec<String>,
}
