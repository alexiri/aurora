//! Serde model of manifest contract v1 (09 §3). Strict: unknown keys fail.
use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_yaml::Value;

pub use aurora_policy::params::{ParamCondition, ParamMatch, ParamSource};

use crate::template::Template;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub package: Package,
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default)]
    pub oauth_clients: Vec<OauthClient>,
    #[serde(default)]
    pub apps: Vec<App>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub id: String,
    pub version: String,
    pub contract: u32,
    pub min_core: String,
    pub publisher: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub homepage: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Hook {
    #[serde(rename = "connect.authorize")]
    ConnectAuthorize,
    #[serde(rename = "connect.exchange")]
    ConnectExchange,
    #[serde(rename = "token.mint")]
    TokenMint,
    #[serde(rename = "approval.summarize")]
    ApprovalSummarize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Permissions {
    #[serde(default)]
    pub hooks: Vec<Hook>,
    #[serde(default)]
    pub egress: BTreeMap<Hook, Vec<String>>,
    #[serde(default)]
    pub context: Vec<String>,
    /// (FIX-107, SEC-17) Hosts of static OAuth token or authorize endpoints that are not also a
    /// claimed host. Exact hosts only; a change is a widening and is shown at approval.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub token_hosts: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClientAuth {
    #[default]
    Body,
    Basic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BodyFormat {
    #[default]
    Form,
    Json,
}

/// A fixed URL, or a template constrained to a zone.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UrlOrTemplate {
    Url(String),
    Templated(ZonedTemplate),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZonedTemplate {
    pub template: Template,
    pub zone: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientField {
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub secret: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OauthClient {
    pub id: String,
    pub authorize_url: UrlOrTemplate,
    pub token_url: UrlOrTemplate,
    #[serde(default)]
    pub pkce: bool,
    #[serde(default)]
    pub client_auth: ClientAuth,
    #[serde(default)]
    pub body_format: BodyFormat,
    #[serde(default)]
    pub fields: Vec<ClientField>,
    pub setup_note: Option<String>,
}

/// The whole-host placeholder of an instance-bound app (09 §3.4 Instance-bound hosts, D-74).
pub const INSTANCE_HOST: &str = "{instance}";

/// `instances` block: the app can be served by self-hosted deployments registered as app
/// instances (D-74).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Instances {
    /// Shown in the registration form and the connect picker.
    pub label: String,
    /// The SaaS host, compiled as an ordinary static claim.
    pub default_host: Option<String>,
    /// Optional reachability check run at registration.
    pub probe: Option<InstanceProbe>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceProbe {
    #[serde(default)]
    pub method: ProbeMethod,
    pub path: String,
    #[serde(default = "default_status")]
    pub expect_status: Vec<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct App {
    pub about: About,
    #[serde(default)]
    pub instances: Option<Instances>,
    #[serde(default)]
    pub credentials: Vec<CredentialDecl>,
    #[serde(default)]
    pub metadata: Vec<MetadataDecl>,
    #[serde(default)]
    pub connect: Vec<Connect>,
    #[serde(default)]
    pub hosts: Vec<HostRule>,
    pub token: Option<Token>,
    pub tools: Option<Tools>,
    #[serde(default)]
    pub approvals: BTreeMap<String, Approval>,
    pub agent: Option<Agent>,
    pub database: Option<Database>,
    #[serde(default)]
    pub grants: Grants,
    /// (phase 4, P4-GSL, D-148, D-149) "this app is a git host that can serve a skill library":
    /// data-only `GET` templates (09 §3.4 `library`); absent = the app cannot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library: Option<Library>,
}

/// `apps[].library` (phase 4, P4-GSL, D-149, SEC-84): three `GET` templates, a repository
/// grammar and JSON-path extractors. Never names a credential or a header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Library {
    /// What an admin may type as the repository (anchored regular expression).
    pub repo_pattern: String,
    /// Exactly one of the app's exact `hosts[]` entries (`{instance}` for instance-bound apps).
    pub request_host: String,
    pub resolve_ref: LibraryResolve,
    pub list_tree: LibraryTree,
    pub get_blob: LibraryBlob,
    #[serde(default)]
    pub required_access: LibraryAccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryAccess {
    #[default]
    Read,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryResolve {
    /// `{ref}` (or `{ref|urlencode}`) → a response carrying the commit sha.
    pub path: String,
    /// Used when no ref is set: the default branch is read from this response.
    pub default_path: String,
    /// JSON path of the commit sha in the `path` response.
    pub sha: String,
    /// JSON path of the default branch name in the `default_path` response.
    pub default_branch: String,
    /// Only `GET` is accepted (`library_method`); absent = `GET`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryPaginate {
    #[default]
    None,
    LinkHeader,
    PageParam,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryTreeFields {
    pub path: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub mode: String,
    pub blob: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryTree {
    /// `{sha}` → the recursive tree of one commit.
    pub path: String,
    /// JSON path of the entries array.
    pub entries: String,
    pub fields: LibraryTreeFields,
    /// The `type` value that marks a regular file entry (`blob`).
    pub file_type: String,
    /// JSON path of the "listing is incomplete" flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<String>,
    #[serde(default)]
    pub paginate: LibraryPaginate,
    /// Upper bound of pages for `link_header` / `page_param`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pages: Option<u32>,
    /// Only `GET` is accepted (`library_method`); absent = `GET`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryBlob {
    /// `{blob}` → one file.
    pub path: String,
    /// JSON path of the content (absent for a `raw` endpoint).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// `raw` or `base64` (literal), or a JSON path of the response's own encoding field.
    pub encoding: String,
    /// Only `GET` is accepted (`library_method`); absent = `GET`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct About {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub api_docs: Option<String>,
    pub api_spec: Option<String>,
    #[serde(default)]
    pub web_hosts: Vec<String>,
    pub connect_note: Option<ConnectNote>,
    pub label_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectNote {
    pub text: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialDecl {
    pub name: String,
    #[serde(default)]
    pub secret: bool,
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataDecl {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Connect {
    Oauth2(Oauth2Connect),
    Fields(FieldsConnect),
    Hook(HookConnect),
}

impl Connect {
    pub fn id(&self) -> &str {
        match self {
            Self::Oauth2(c) => &c.id,
            Self::Fields(c) => &c.id,
            Self::Hook(c) => &c.id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Callback {
    #[default]
    Query,
    Fragment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    Read,
    Write,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeDecl {
    pub scope: String,
    pub name: String,
    pub access: Access,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessLevel {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub authorize_params: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Oauth2Connect {
    pub id: String,
    pub client: String,
    #[serde(default)]
    pub scopes: Vec<ScopeDecl>,
    #[serde(default)]
    pub default_scopes: Vec<String>,
    #[serde(default)]
    pub authorize_params: BTreeMap<String, String>,
    #[serde(default)]
    pub access_levels: Vec<AccessLevel>,
    #[serde(default)]
    pub callback: Callback,
    #[serde(default)]
    pub token_response: BTreeMap<String, String>,
    /// Connect-form fields (S-14); only `store: metadata | tenant_host` is allowed.
    #[serde(default)]
    pub fields: Vec<FormField>,
    pub probe: Option<Probe>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldsConnect {
    pub id: String,
    pub fields: Vec<FormField>,
    pub probe: Option<Probe>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookConnect {
    pub id: String,
    #[serde(default)]
    pub authorize: bool,
    #[serde(default)]
    pub fields: Vec<FormField>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    #[default]
    Text,
    Secret,
    Select,
    File,
    JsonFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Store {
    #[default]
    Credentials,
    Metadata,
    TenantHost,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShowWhen {
    pub field: String,
    pub equals: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormField {
    pub name: String,
    pub label: String,
    #[serde(rename = "type", default)]
    pub kind: FieldType,
    #[serde(default)]
    pub store: Store,
    #[serde(default)]
    pub required: bool,
    pub placeholder: Option<String>,
    pub help_url: Option<String>,
    #[serde(default)]
    pub options: Vec<SelectOption>,
    pub show_when: Option<ShowWhen>,
    #[serde(default)]
    pub json_map: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ProbeMethod {
    #[default]
    Get,
    Head,
    Post,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Probe {
    #[serde(default)]
    pub method: ProbeMethod,
    pub url: Template,
    #[serde(default = "default_status")]
    pub expect_status: Vec<u16>,
    pub label: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

fn default_status() -> Vec<u16> {
    vec![200]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostMatch {
    pub host: String,
    pub path_prefix: Option<String>,
}

/// Inject-stage mechanisms (09 §5). Unknown → deserialization error.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Inject {
    Bearer(Template),
    Basic {
        user: Template,
        password: Template,
    },
    Header {
        name: String,
        value: Template,
    },
    Query {
        name: String,
        value: Template,
    },
    RemoveHeader(String),
    PathTemplate {
        prefix: String,
        value: Template,
    },
    PathRegex {
        pattern: String,
        replacement: Template,
    },
}

/// route stage: `rewrite_host: { template, zone }`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RewriteHost {
    pub template: Template,
    pub zone: String,
}

/// guard stage: `tenant: { field, zone }`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tenant {
    pub field: String,
    pub zone: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AwsSigv4 {
    pub region: Template,
    pub service: String,
    pub access_key: Template,
    pub secret_key: Template,
    pub session_token: Option<Template>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Finalize {
    AwsSigv4(AwsSigv4),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitTrailer {
    pub trailer: Template,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transform {
    GitCommitTrailer(CommitTrailer),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenEndpoint {
    pub path: String,
    #[serde(default = "post")]
    pub method: String,
}

fn post() -> String {
    "POST".to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intercept {
    OauthTokenEndpoint(TokenEndpoint),
}

/// passthrough stage: `passthrough: [registry_token_challenge]`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Passthrough {
    RegistryTokenChallenge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostRule {
    #[serde(rename = "match")]
    pub matcher: HostMatch,
    #[serde(default, deserialize_with = "de_vec")]
    pub inject: Vec<Inject>,
    pub tenant: Option<Tenant>,
    pub rewrite_host: Option<RewriteHost>,
    #[serde(default, deserialize_with = "de_opt")]
    pub finalize: Option<Finalize>,
    #[serde(default, deserialize_with = "de_opt")]
    pub transform: Option<Transform>,
    #[serde(default, deserialize_with = "de_opt")]
    pub intercept: Option<Intercept>,
    #[serde(default)]
    pub passthrough: Vec<Passthrough>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenKind {
    #[default]
    Static,
    Oauth2Refresh,
    Hook,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaleCode {
    pub status: u16,
    pub body_contains: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Token {
    #[serde(default)]
    pub kind: TokenKind,
    pub client: Option<String>,
    pub refresh_field: Option<String>,
    #[serde(default)]
    pub rotate_refresh_token: bool,
    pub max_lifetime_secs: Option<u64>,
    #[serde(default)]
    pub extra_params: BTreeMap<String, Template>,
    #[serde(default)]
    pub stale_codes: Vec<StaleCode>,
    #[serde(default)]
    pub reauth_codes: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tools {
    pub read: Option<ToolGroup>,
    pub write: Option<ToolGroup>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolGroup {
    pub wildcard: Option<Wildcard>,
    #[serde(default)]
    pub items: Vec<ToolItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wildcard {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphqlKind {
    Query,
    Mutation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GitKind {
    Fetch,
    Push,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolItem {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub host: String,
    pub path: String,
    #[serde(default)]
    pub methods: Vec<String>,
    pub graphql: Option<GraphqlKind>,
    pub git: Option<GitKind>,
    /// (phase 4, P4-GRANTS, Q8, D-124) 1–4 AND-ed parameter conditions (a classifier beside
    /// `graphql` and `git`); absent = matched on host, method and path only, as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Vec<ParamCondition>>,
}

/// `apps[].grants` (phase 4, P4-GRANTS, D-123, SEC-67): restrictions that only tighten.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grants {
    /// `full` grants are refused for the app; the evaluator never lets a toolless target allow it.
    #[serde(default)]
    pub custom_only: bool,
    /// A request on the app's winning connection that matches no catalogue tool is blocked.
    #[serde(default)]
    pub block_untooled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    pub title: String,
    #[serde(default)]
    pub fields: Vec<SummaryField>,
    pub link: Option<Template>,
    #[serde(default)]
    pub summarize: Option<SummarizeMode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummarizeMode {
    Hook,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryField {
    pub label: String,
    #[serde(flatten)]
    pub source: Source,
}

/// Summary value source (09 §3.4 `approvals`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum Source {
    Path {
        segment: u32,
    },
    Query {
        name: String,
    },
    Header {
        name: String,
    },
    Body {
        #[serde(deserialize_with = "de_vec")]
        steps: Vec<Decoder>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphqlSelect {
    OperationName,
    FieldNames,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoParams {}

/// Core decoders (09 §5). Unknown → deserialization error.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decoder {
    Json(String),
    Form(String),
    Graphql(GraphqlSelect),
    Base64url(NoParams),
    Base64(NoParams),
    MimeHeader(String),
    MimeText(NoParams),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvVar {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stub {
    pub path: String,
    #[serde(default)]
    pub mode: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub env: Vec<EnvVar>,
    #[serde(default)]
    pub stubs: Vec<Stub>,
    #[serde(default)]
    pub default_blocklist: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Database {
    pub engine: String,
}

/// serde_yaml 0.9 reads enums from `!tag` only; the contract writes them as
/// single-key maps (`bearer: ...`). Re-tag the value, then decode.
fn retag<T: DeserializeOwned>(v: Value) -> Result<T, serde_yaml::Error> {
    let v = match v {
        Value::Mapping(m) if m.len() == 1 => match m.into_iter().next() {
            Some((Value::String(k), val)) => {
                Value::Tagged(Box::new(serde_yaml::value::TaggedValue {
                    tag: serde_yaml::value::Tag::new(k),
                    value: val,
                }))
            }
            Some((k, val)) => Value::Mapping([(k, val)].into_iter().collect()),
            None => Value::Null,
        },
        other => other,
    };
    serde_yaml::from_value(v)
}

pub(crate) fn de_vec<'de, D: Deserializer<'de>, T: DeserializeOwned>(
    d: D,
) -> Result<Vec<T>, D::Error> {
    Vec::<Value>::deserialize(d)?
        .into_iter()
        .map(|v| retag(v).map_err(serde::de::Error::custom))
        .collect()
}

pub(crate) fn de_opt<'de, D: Deserializer<'de>, T: DeserializeOwned>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::<Value>::deserialize(d)?
        .map(|v| retag(v).map_err(serde::de::Error::custom))
        .transpose()
}
