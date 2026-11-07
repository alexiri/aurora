//! Serde model of manifest contract v1 (09 §3). Strict: unknown keys fail.
use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_yaml::Value;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct App {
    pub about: About,
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
