//! Serde model of manifest contract v1 (09 §3). Strict: unknown keys fail.
use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_yaml::Value;

pub use aurora_policy::params::{ParamCondition, ParamMatch, ParamSource};

use crate::manifest::BodyCheck;
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
    /// (FIX-233, C-1159) The value used when a client leaves the field out; such a field is
    /// optional when a client is registered. Required for a field an OAuth URL places in its
    /// path (`{client.<name>}`); never on a secret field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
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
    /// (FIX-134, D-243) Hosts the package ships as ready-made instances next to `default_host`
    /// (a vendor's region hosts): static claims signed with the package, offered in the connect
    /// window with no registration. Each is served like a registered instance.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub presets: Vec<InstancePreset>,
    /// Optional reachability check run at registration.
    pub probe: Option<InstanceProbe>,
}

/// One preset instance of an `instances` block (FIX-134, D-243).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstancePreset {
    /// A plain lower-case DNS name inside a registrable domain, distinct from `default_host`.
    pub host: String,
    /// Shown in the connect picker ("US region").
    pub label: String,
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
    /// (phase 5, D-251) the gateway LDAP lane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ldap: Option<Ldap>,
    /// (phase 5, P5-ICS, D-273) the secret-link lane: the credential is the destination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_link: Option<SecretLink>,
    /// (phase 5, P5-CALDAV, D-275) person-pinned servers: the connection, not the package, names the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub own_server: Option<OwnServer>,
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
    /// (P5-POLISH-3, D-387, C-1031) One plain sentence, 1 to 100 characters, saying what an agent can do
    /// with the app; cards show it whole instead of cutting `description`. Presentation only: changing it
    /// never widens permissions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// (P5-POLISH-3, D-387, C-1031) A question a person could send to try the app, 1 to 120 characters
    /// ("What's on my calendar this week?"); the journey's Done step opens it as a chat draft.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_prompt: Option<String>,
    /// (FIX-135, D-242) The product this app is a deployment of, shared with the other apps of
    /// the package that are the same product elsewhere (a vendor's hosted service and its
    /// self-hosted server): the catalogue and every picker show one product. Presentation only;
    /// the apps keep their own hosts, tools and token kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<Family>,
}

/// `about.family` (FIX-135, D-242).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Family {
    /// Lower-case slug, the same on every member.
    pub id: String,
    /// The product name shown first, identical on every member.
    pub name: String,
    /// This member's deployment, unique within the family (its host, "Self-hosted", "Data Center").
    pub variant_label: String,
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
    /// (FIX-136) The scopes the method can grant or let a person declare (none for a hook).
    pub fn declared_scopes(&self) -> &[ScopeDecl] {
        match self {
            Self::Oauth2(c) => &c.scopes,
            Self::Fields(c) => &c.scopes,
            Self::Hook(_) => &[],
        }
    }

    pub fn id(&self) -> &str {
        match self {
            Self::Oauth2(c) => &c.id,
            Self::Fields(c) => &c.id,
            Self::Hook(c) => &c.id,
        }
    }

    /// (FIX-123, D-195, D-214) Short method name shown in the connect window's chooser.
    pub fn label(&self) -> Option<&str> {
        match self {
            Self::Oauth2(c) => c.label.as_deref(),
            Self::Fields(c) => c.label.as_deref(),
            Self::Hook(c) => c.label.as_deref(),
        }
    }

    /// One or two sentences: what the method is and when to use it.
    pub fn description(&self) -> Option<&str> {
        match self {
            Self::Oauth2(c) => c.description.as_deref(),
            Self::Fields(c) => c.description.as_deref(),
            Self::Hook(c) => c.description.as_deref(),
        }
    }

    /// (P5-UX-2, C-956) Who the method is for; `Admin` is never suggested.
    pub fn audience(&self) -> Audience {
        match self {
            Self::Oauth2(c) => c.audience,
            Self::Fields(c) => c.audience,
            Self::Hook(c) => c.audience,
        }
    }

    /// (P5-UX-13, C-942) Context values the person must copy into the other service.
    pub fn show_context(&self) -> &[String] {
        match self {
            Self::Oauth2(c) => &c.show_context,
            Self::Fields(c) => &c.show_context,
            Self::Hook(c) => &c.show_context,
        }
    }

    /// (P5-UX-2, C-956) The scopes of the journey's choices: the declared `scope_presets`, else
    /// `read` = the method's read scopes and `write` = all of its scopes.
    pub fn effective_scope_presets(&self) -> ScopePresets {
        let declared = match self {
            Self::Oauth2(c) => c.scope_presets.clone(),
            Self::Fields(c) => c.scope_presets.clone(),
            Self::Hook(_) => None,
        };
        declared.unwrap_or_else(|| {
            let all = self.declared_scopes();
            ScopePresets {
                read: all
                    .iter()
                    .filter(|d| d.access == Access::Read)
                    .map(|d| d.scope.clone())
                    .collect(),
                write: all.iter().map(|d| d.scope.clone()).collect(),
            }
        })
    }

    /// (P5-POLISH-3, C-1031) The method's access levels: an OAuth method's account choices, a token
    /// method's `read` / `write` choices with their `permission_hints` (none for a hook).
    pub fn access_levels(&self) -> &[AccessLevel] {
        match self {
            Self::Oauth2(c) => &c.access_levels,
            Self::Fields(c) => &c.access_levels,
            Self::Hook(_) => &[],
        }
    }

    /// The method the connect window preselects (at most one per app).
    pub fn recommended(&self) -> bool {
        match self {
            Self::Oauth2(c) => c.recommended,
            Self::Fields(c) => c.recommended,
            Self::Hook(c) => c.recommended,
        }
    }

    /// The `token` of this method, overriding the app's for connections made with it.
    pub fn token(&self) -> Option<&Token> {
        match self {
            Self::Oauth2(c) => c.token.as_ref(),
            Self::Fields(c) => c.token.as_ref(),
            Self::Hook(c) => c.token.as_ref(),
        }
    }

    /// The form fields of the method (the credentials and metadata it produces, with `store`).
    pub fn form_fields(&self) -> &[FormField] {
        match self {
            Self::Oauth2(c) => &c.fields,
            Self::Fields(c) => &c.fields,
            Self::Hook(c) => &c.fields,
        }
    }
}

/// (FIX-123) The `token` of connections made with `method`: the method's own when it declares
/// one, else the app's. `None` (a connection with no recorded method) = the app's.
pub fn token_for<'a>(
    connect: &'a [Connect],
    app_token: Option<&'a Token>,
    method: Option<&str>,
) -> Option<&'a Token> {
    method
        .and_then(|m| connect.iter().find(|c| c.id() == m))
        .and_then(Connect::token)
        .or(app_token)
}

impl App {
    pub fn token_for(&self, method: Option<&str>) -> Option<&Token> {
        token_for(&self.connect, self.token.as_ref(), method)
    }
}

impl ToolItem {
    /// (FIX-123) Does the tool apply to connections made with `method`? A tool without
    /// `connect_methods` applies to every method; so does any tool to a connection that
    /// records no method (unknown: nothing is hidden).
    pub fn applies_to(&self, method: Option<&str>) -> bool {
        self.connect_methods.is_empty()
            || method.is_none_or(|m| self.connect_methods.iter().any(|x| x == m))
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
    /// (P5-POLISH-3, D-387, C-1031) The permissions to tick on the app's own token screen for this
    /// level, in the app's own words ("Contents · Read"): 1 to 60 characters each, at most 12. On a
    /// `fields` (token) method the levels are the journey's choices, `read` and `write`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permission_hints: Vec<String>,
}

/// (P5-UX-2, C-956) Who a connect method is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Audience {
    #[default]
    Person,
    Admin,
}

impl Audience {
    fn is_person(&self) -> bool {
        *self == Self::Person
    }
}

/// (P5-UX-2, C-956) The scopes each journey choice requests; each is one of the method's scopes,
/// and `read` holds only `access: read` ones.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopePresets {
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub write: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Oauth2Connect {
    pub id: String,
    /// (FIX-123, D-214) Presentation in the connect window's method chooser, and the `token`
    /// that overrides the app's for connections made with this method.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub recommended: bool,
    /// (P5-UX-2, C-956) Who can use the method: `admin` needs material only an administrator
    /// holds, so the journey never suggests it.
    #[serde(default, skip_serializing_if = "Audience::is_person")]
    pub audience: Audience,
    /// (P5-UX-2, C-956) Scopes of the journey's "Read only" and "Read and change" choices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_presets: Option<ScopePresets>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<Token>,
    /// (P5-UX-13, C-942) Values the person copies into the other service.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub show_context: Vec<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub recommended: bool,
    /// (P5-UX-2, C-956) Who can use the method: `admin` needs material only an administrator
    /// holds, so the journey never suggests it.
    #[serde(default, skip_serializing_if = "Audience::is_person")]
    pub audience: Audience,
    /// (P5-UX-2, C-956) Scopes of the journey's "Read only" and "Read and change" choices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_presets: Option<ScopePresets>,
    /// (P5-POLISH-3, C-1031) The journey's `read` and `write` choices with the token permissions to
    /// tick for each (`permission_hints`); no other id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub access_levels: Vec<AccessLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<Token>,
    /// (P5-UX-13, C-942) Values the person copies into the other service.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub show_context: Vec<String>,
    pub fields: Vec<FormField>,
    pub probe: Option<Probe>,
    /// (FIX-136, D-241) The scopes a pasted token may carry, which the connect form asks the
    /// person to declare ("Which scopes did you give this token?"); stored as the connection's
    /// `scopes` and trusted only to *refuse* a tool the token cannot serve, never to widen.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<ScopeDecl>,
    /// (FIX-136) Read probes that prefill the declaration: one entry per **read** scope; a probe
    /// answering one of its `expect_status` ticks the scope. Write scopes are never probed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub probe_scopes: BTreeMap<String, Probe>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookConnect {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub recommended: bool,
    /// (P5-UX-2, C-956) Who can use the method: `admin` needs material only an administrator
    /// holds, so the journey never suggests it.
    #[serde(default, skip_serializing_if = "Audience::is_person")]
    pub audience: Audience,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<Token>,
    /// (P5-UX-13, C-942) Values the person copies into the other service.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub show_context: Vec<String>,
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
    /// (FIX-194, D-420, C-1091) The value is the connection's label; it is stored nowhere else.
    Label,
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

fn is_false(b: &bool) -> bool {
    !*b
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
    /// (FIX-123, D-214) Connect method ids this tool works with; empty = every method. The
    /// brief's `tools[].methods` is spelled `connect_methods` here: `methods` is the HTTP
    /// methods of the tool.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connect_methods: Vec<String>,
    /// (FIX-136, D-241) Scope ids (any of) the tool needs; empty = no requirement. Ids come from
    /// the scopes of the app's connect methods (`oauth2` `scopes`, `fields` `scopes`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    pub graphql: Option<GraphqlKind>,
    pub git: Option<GitKind>,
    /// (phase 4, P4-GRANTS, Q8, D-124) 1–4 AND-ed parameter conditions (a classifier beside
    /// `graphql` and `git`); absent = matched on host, method and path only, as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Vec<ParamCondition>>,
    /// (P5-TOOLASK, D-397, C-1042) The package's default for this tool: `ask` = held for a person's
    /// approval unless an explicit workspace or organization rule allows or blocks it. Write group
    /// only; absent = no package default (and no change to the package's snapshot hash).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ToolDefaultApproval>,
    /// A request-body shape the gateway enforces for this tool's raw sends: a body that is
    /// not the declared shape is refused before any rule, approval or injection, so nothing
    /// the approval card cannot show is ever held for a person's approval. Absent = no
    /// body check (and no change to the package's snapshot hash).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_check: Option<BodyCheck>,
    /// The tool declares no query parameters: a request that carries a query string is
    /// refused before any rule, approval or injection (a parameter could make the answer
    /// carry what the tool's description says never reaches the agent, an alert server's
    /// `decrypt=true` on contact points). Absent = the query is not checked.
    #[serde(default, skip_serializing_if = "is_false")]
    pub refuse_query: bool,
    /// (lane review 2026-11-25) Response projection: of every item of the tool's JSON array
    /// answer (or of the object itself) only these `$`-paths stay, applied by the gateway on
    /// the response path for every caller; an answer that is not JSON in the projected shape
    /// is withheld. The agent CLI reduces what it saves and prints the same way. Absent = the
    /// answer is forwarded as it arrived (and no change to the package's snapshot hash).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<Vec<String>>,
}

/// (P5-TOOLASK, C-1042) The values of a tool's `approval`; only `ask` exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolDefaultApproval {
    Ask,
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
    /// (FIX-118) Number of attachments of an RFC 5322 message, as text.
    MimeAttachments(NoParams),
    /// (FIX-118) Number of elements of the JSON array at this pointer (`0` when it is absent).
    JsonCount(String),
    /// (FIX-118) Every `text`/`content` string below the JSON value at this pointer, joined with
    /// spaces (nested document and rich-text block formats).
    JsonText(String),
    /// (P5-CALDAV, D-275) Every value of this property (`SUMMARY`, `DTSTART`, `ATTENDEE`, …) of the
    /// iCalendar body's events, joined with `, `; a `TZID` is appended to a date-time and an
    /// attendee's name leads its address.
    Ical(String),
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

/// (P5-GUIDE-1, C-640) One `agent.guide[]` entry of a submitted manifest. Since P5-APPSKILL-1
/// (D-453) the key is only parsed, so an old manifest still compiles; the linter warns
/// `agent_guide_ignored` and nothing of it is used. The fields stay permissive for that reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuideEntry {
    pub file: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    /// (P5-GUIDE-1, `docs/24 §3.1`) One line of what the app is for; the only plugin text in the
    /// instruction file. Required when `guide` is present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub instructions: String,
    /// (P5-GUIDE-1) Reference files and recipes of `agent/<app_id>/`, in listing order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guide: Vec<GuideEntry>,
    /// (P5-GUIDE-6, `docs/24 §3.5`, C-640) One `.json` file of `agent/<app_id>/` with the app's
    /// HTTP operations (C-651); it is not listed in `guide`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operations: Option<String>,
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

/// (phase 5, P5-CERN-ADDONS, D-251) "this app reads a directory over LDAP through the gateway
/// LDAP lane" (`docs/06 §LDAP lane`, `docs/09 §3 ldap`). Anonymous bind, search only, LDAPS only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ldap {
    /// Exact public DNS name of the directory server.
    pub server: String,
    /// LDAPS port; default 636.
    #[serde(default = "default_ldaps_port")]
    pub port: u16,
    /// Every search is confined to this subtree.
    pub base_dn: String,
    /// The only attributes a request may ask for and a result may carry.
    pub attributes: Vec<String>,
    /// Most entries one search returns (default 50, at most 500).
    #[serde(default = "default_ldap_max_entries")]
    pub max_entries: u32,
}

/// (phase 5, P5-ICS, D-273, C-843, SEC-107) "this app's credential is a URL the person typed, and the
/// gateway fetches exactly that URL" (`docs/09 §3.4 secret_link`). The app has no `hosts[]` entry;
/// its tools are `GET` on [`SECRET_LINK_TOOL_HOST`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretLink {
    /// Name of the declared `secret: true` credential that holds the URL.
    pub credential: String,
    /// What the body must start with at connect (≤ 64 characters), e.g. `BEGIN:VCALENDAR`.
    /// Refused in `send` mode (there is no read at connect).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect_prefix: Option<String>,
    /// (P5-WEBHOOK-1, D-320, C-909) `fetch` (default): the gateway reads the stored URL.
    /// `send`: the gateway posts the person's rendered template to it (an outgoing webhook).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<SecretLinkMode>,
    /// (C-909) `send` mode: archive files `presets/<file>`, each a `WebhookPreset` JSON. Refused in
    /// `fetch` mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presets: Option<Vec<String>>,
}

/// (P5-WEBHOOK-1, C-909) What a secret-link app does with its URL.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretLinkMode {
    #[default]
    Fetch,
    Send,
}

/// Most presets one send-mode app lists.
pub const SECRET_LINK_PRESETS_MAX: usize = 20;
/// Archive directory of the webhook preset files (`presets/<file>.json`).
pub const PRESETS_DIR: &str = "presets/";

impl SecretLink {
    /// The effective mode (`fetch` when absent).
    #[must_use]
    pub fn mode(&self) -> SecretLinkMode {
        self.mode.unwrap_or_default()
    }
}

/// A preset file name: `^[a-z0-9][a-z0-9-]{0,40}\.json$`.
#[must_use]
pub fn preset_file_ok(f: &str) -> bool {
    let Some(stem) = f.strip_suffix(".json") else {
        return false;
    };
    let b = stem.as_bytes();
    !b.is_empty()
        && b.len() <= 41
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// (phase 5, P5-CALDAV, D-275, C-845, SEC-109) "the person points their own credential at a server
/// they choose" (`docs/09 §3.4 own_server`). The app has no `hosts[]` entry: the connection stores
/// the server's origins (`hosts_field`, metadata: `host:port` strings) and the discovered `url`
/// (`url_field`); the credential is injected only on those exact origins, over TLS.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnServer {
    /// Metadata field holding the server's calendar-home URL (shown to the agent).
    pub url_field: String,
    /// Metadata field holding the pinned origins (`host:port`, at most [`OWN_SERVER_HOSTS_MAX`]).
    pub hosts_field: String,
    /// Inject stages (only `basic`, `bearer`, `header`).
    #[serde(deserialize_with = "de_vec")]
    pub inject: Vec<Inject>,
}

/// The whole-host placeholder of a tool of an `own_server` app: any origin the connection pinned.
pub const CONNECTION_HOST: &str = "{connection}";
/// Most origins one own-server connection may pin.
pub const OWN_SERVER_HOSTS_MAX: usize = 4;

/// The synthetic host suffix of the secret-link lane: `<connection_id>.link.aurora.internal`.
pub const SECRET_LINK_HOST_SUFFIX: &str = ".link.aurora.internal";
/// The tool host pattern of a secret-link app.
pub const SECRET_LINK_TOOL_HOST: &str = "*.link.aurora.internal";
/// Longest `expect_prefix`.
pub const SECRET_LINK_PREFIX_MAX: usize = 64;

impl SecretLink {
    /// The synthetic request host of one connection.
    #[must_use]
    pub fn host_of(connection_id: impl std::fmt::Display) -> String {
        format!("{connection_id}{SECRET_LINK_HOST_SUFFIX}")
    }

    /// The label before the suffix of a synthetic host (the caller parses it as a connection id).
    #[must_use]
    pub fn id_label(host: &str) -> Option<&str> {
        let n = host.len().checked_sub(SECRET_LINK_HOST_SUFFIX.len())?;
        (host.is_char_boundary(n) && host[n..].eq_ignore_ascii_case(SECRET_LINK_HOST_SUFFIX))
            .then(|| &host[..n])
    }
}

pub const LDAP_HOST_SUFFIX: &str = ".ldap.aurora.internal";
pub const LDAP_MAX_ENTRIES: u32 = 500;
pub const LDAP_MAX_ATTRIBUTES: usize = 64;

fn default_ldaps_port() -> u16 {
    636
}

fn default_ldap_max_entries() -> u32 {
    50
}

impl Ldap {
    /// The synthetic request host of an LDAP app: `<app id>.ldap.aurora.internal`.
    #[must_use]
    pub fn host_of(app_id: &str) -> String {
        format!("{app_id}{LDAP_HOST_SUFFIX}")
    }
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
