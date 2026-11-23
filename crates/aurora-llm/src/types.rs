use crate::{Endpoint, LlmError, Result};
use aurora_core::Secret;
use http::{HeaderMap, Method};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dialect {
    AnthropicMessages,
    OpenaiChat,
    OpenaiResponses,
    /// (P4-LLM, D-260(a)) Anthropic Messages as Bedrock serves them (`invoke`/`converse` under `/model/{id}/…`).
    AnthropicBedrock,
    /// (P4-LLM) Anthropic Messages as Vertex serves them (`rawPredict`).
    AnthropicVertex,
}

impl Dialect {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "anthropic_messages" => Some(Self::AnthropicMessages),
            "openai_chat" => Some(Self::OpenaiChat),
            "openai_responses" => Some(Self::OpenaiResponses),
            "anthropic_bedrock" => Some(Self::AnthropicBedrock),
            "anthropic_vertex" => Some(Self::AnthropicVertex),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AnthropicMessages => "anthropic_messages",
            Self::OpenaiChat => "openai_chat",
            Self::OpenaiResponses => "openai_responses",
            Self::AnthropicBedrock => "anthropic_bedrock",
            Self::AnthropicVertex => "anthropic_vertex",
        }
    }
}

/// How a secret authenticates to its provider (`08 §LLM secrets`, C-801). The value shape of each mode is in C-802.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuthMode {
    ApiKey,
    /// Static AWS IAM key (Bedrock).
    AwsSigv4,
    /// Source IAM key + role; the gateway assumes the role (D-261(b)).
    AwsAssumeRole,
    /// A Google service-account key file (Vertex).
    GcpServiceAccount,
    /// An Entra app, client-credentials grant (Azure, D-261(c)).
    AzureEntra,
}

impl AuthMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "api_key" => Some(Self::ApiKey),
            "aws_sigv4" => Some(Self::AwsSigv4),
            "aws_assume_role" => Some(Self::AwsAssumeRole),
            "gcp_service_account" => Some(Self::GcpServiceAccount),
            "azure_entra" => Some(Self::AzureEntra),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::AwsSigv4 => "aws_sigv4",
            Self::AwsAssumeRole => "aws_assume_role",
            Self::GcpServiceAccount => "gcp_service_account",
            Self::AzureEntra => "azure_entra",
        }
    }
    /// The shape of the secret's value (C-802 `value_shape`): `string` for a bare key, else the JSON shape's name.
    pub fn value_shape(self) -> &'static str {
        match self {
            Self::ApiKey => "string",
            other => other.as_str(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointKind {
    Fixed(&'static str),
    Configured,
    /// (P4-LLM-2) No `base_url` in the entry: the origin follows other keys (Vertex: `location`).
    Derived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OriginMethod {
    Endpoint,
    LitellmTeam,
    ProviderAccount,
}

impl OriginMethod {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "endpoint" => Some(Self::Endpoint),
            "litellm_team" => Some(Self::LitellmTeam),
            "provider_account" => Some(Self::ProviderAccount),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Endpoint => "endpoint",
            Self::LitellmTeam => "litellm_team",
            Self::ProviderAccount => "provider_account",
        }
    }
}

/// (P4-LLM-4) What a provider needs to know about the secret it validates besides the key: the auth mode and, for
/// `aws_assume_role`, the organization (`RoleSessionName`) and its own `external_id` (`ExternalId`, D-261).
#[derive(Debug, Clone)]
pub struct KeyCheck {
    pub mode: AuthMode,
    pub org_id: String,
    pub org_external_id: Option<String>,
}

impl KeyCheck {
    #[must_use]
    pub fn new(mode: AuthMode) -> Self {
        Self {
            mode,
            org_id: String::new(),
            org_external_id: None,
        }
    }
}

/// Account identifiers a provider reports for a key; every field optional.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyOrigin {
    pub organization_id: Option<String>,
    pub workspace_id: Option<String>,
    pub team_id: Option<String>,
    pub user_id: Option<String>,
}

/// Effective key-origin rule of an endpoint (`[llm.endpoints.key_origin]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyOriginRule {
    pub method: OriginMethod,
    pub allowed_organization_ids: Vec<String>,
    pub allowed_workspace_ids: Vec<String>,
    pub allowed_team_ids: Vec<String>,
}

impl KeyOriginRule {
    pub fn from_config(c: &aurora_config::KeyOrigin) -> Result<Self> {
        let method = OriginMethod::parse(&c.method)
            .ok_or_else(|| LlmError::Config(format!("unknown key_origin method `{}`", c.method)))?;
        Ok(Self {
            method,
            allowed_organization_ids: c.allowed_organization_ids.clone(),
            allowed_workspace_ids: c.allowed_workspace_ids.clone(),
            allowed_team_ids: c.allowed_team_ids.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: Option<String>,
}

/// Which model a request asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelRef {
    Model(String),
    /// The request uses no model (e.g. listing models).
    NoModel,
    /// Body truncated/unparsable/missing model: fail closed.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectionHeader {
    /// `Authorization: Bearer <key>`
    Bearer,
    /// `x-api-key: <key>`
    XApiKey,
    /// `<name>: <key>` for a lowercase header name other than the two above (`api-key`).
    Header(&'static str),
    /// (C-806; implemented by P4-LLM-4) SigV4 with the secret's IAM key.
    AwsSigV4,
    /// (P4-LLM-4) SigV4 with session credentials minted by assuming the role.
    AwsAssumeRole,
    /// (P4-LLM-2) `Authorization: Bearer <Google access token minted from the service account>`.
    GcpToken,
    /// (P4-LLM-3) `Authorization: Bearer <Entra token>`.
    EntraToken,
}

impl InjectionHeader {
    /// Recipes that put the secret's value into one header as is. The others need a mint or a signature, which the
    /// provider lanes add: the gateway refuses them with 502 `credential_unavailable` until then.
    pub fn is_plain(self) -> bool {
        matches!(self, Self::Bearer | Self::XApiKey | Self::Header(_))
    }
}

/// The recipe of C-806 for one provider and auth mode: which header carries the credential, which agent-supplied
/// headers are removed first (never merged), and fixed non-secret headers the gateway sets (OpenAI organization).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmInjection {
    pub header: InjectionHeader,
    /// Agent-supplied request headers to remove (lowercase).
    pub strip_request_headers: Vec<&'static str>,
    /// Agent-supplied request headers removed by prefix (lowercase, e.g. `x-amz-`, `x-goog-`).
    pub strip_request_prefixes: Vec<&'static str>,
    /// Non-secret headers the gateway sets from the endpoint entry (after the strip).
    pub set_headers: Vec<(&'static str, String)>,
}

impl LlmInjection {
    pub fn new(header: InjectionHeader, strip: &[&'static str]) -> Self {
        Self {
            header,
            strip_request_headers: strip.to_vec(),
            strip_request_prefixes: Vec::new(),
            set_headers: Vec::new(),
        }
    }
}

#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn endpoint_kind(&self) -> EndpointKind;
    fn auth_modes(&self) -> &'static [AuthMode];
    fn dialects(&self, ep: &Endpoint) -> Vec<Dialect>;
    /// Host patterns (`host` or `host:port`) of the endpoint.
    fn hosts(&self, ep: &Endpoint) -> Vec<String>;
    /// The base URL the gateway sends the endpoint's requests to (FIX-104: a key is injected
    /// only to exactly this scheme, host and port).
    fn base_url(&self, ep: &Endpoint) -> String {
        ep.base_url.clone()
    }
    fn injection(&self, mode: &AuthMode) -> LlmInjection;
    /// The recipe for one endpoint (C-806): providers whose headers come from the endpoint entry (OpenAI organization,
    /// the compatible `auth_header`) override this; the rest use [`Self::injection`].
    fn injection_for(&self, ep: &Endpoint, mode: &AuthMode) -> LlmInjection {
        let _ = ep;
        self.injection(mode)
    }
    /// (SEC-65) Whether the sanctioned key may be injected into this request (the provider's inference path set plus
    /// the endpoint's `allowed_paths`). Default: the `litellm` set.
    fn inference_path_permitted(&self, ep: &Endpoint, method: &Method, path: &str) -> bool {
        crate::inference_path_permitted(method, path, &ep.allowed_paths)
    }
    /// (C-808) The error class of an upstream answer; every provider shares the one table.
    fn classify_error(&self, status: u16, error_type: Option<&str>) -> crate::LlmErrorClass {
        crate::classify(self.id(), status, error_type)
    }
    async fn list_models(
        &self,
        ep: &Endpoint,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<Vec<ModelInfo>>;
    /// (FIX-105, CRED-6) The provider's own answer, without the static-list fallback of
    /// [`Self::list_models`]: key verification must see an unreachable endpoint as unreachable.
    async fn list_models_strict(
        &self,
        ep: &Endpoint,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<Vec<ModelInfo>> {
        self.list_models(ep, key, http).await
    }
    /// (P4-LLM-4) [`Self::list_models`] for a secret of the auth mode in `check` (Bedrock signs its listing with the
    /// secret's credentials, so it needs the mode and, for `aws_assume_role`, the organization's external id).
    async fn list_models_as(
        &self,
        ep: &Endpoint,
        check: &KeyCheck,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<Vec<ModelInfo>> {
        let _ = check;
        self.list_models(ep, key, http).await
    }
    /// (D-372, D-474) The `mode` of each public model name, from a provider route that carries it (LiteLLM's
    /// `GET /model_group/info`, then `GET /model/info`), so an automatic pick can skip embedding, image and
    /// audio models. A name the answer does not cover is a chat model. An empty map means no mode is known
    /// (providers without such a route, or a key the route tells nothing).
    async fn model_modes(
        &self,
        ep: &Endpoint,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<std::collections::HashMap<String, String>> {
        let _ = (ep, key, http);
        Ok(std::collections::HashMap::new())
    }
    /// (P4-LLM-3, P4-LLM-4) [`Self::list_models_strict`] for a secret of the auth mode in `check` (providers with several
    /// value shapes, Azure's and Bedrock's, override it; the others ignore it).
    async fn list_models_strict_as(
        &self,
        ep: &Endpoint,
        check: &KeyCheck,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<Vec<ModelInfo>> {
        let _ = check;
        self.list_models_strict(ep, key, http).await
    }
    /// (P4-LLM-3, P4-LLM-4) [`Self::verify_key_origin`] for a secret of the auth mode in `check`.
    async fn verify_key_origin_as(
        &self,
        ep: &Endpoint,
        check: &KeyCheck,
        method: OriginMethod,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<KeyOrigin> {
        let _ = check;
        self.verify_key_origin(ep, method, key, http).await
    }
    /// (P4-LLM-2) Whether the key is validated by the origin proof itself (Vertex: the mint that proves the project and
    /// e-mail is also the validation), so the API makes one call, not a listing and a proof.
    fn validates_by_origin(&self) -> bool {
        false
    }
    fn default_model(&self, ep: &Endpoint) -> Option<String>;
    /// Placeholder-only environment for a sandboxed client.
    fn client_env(&self, ep: &Endpoint, dialect: Dialect, mode: &AuthMode)
    -> Vec<(String, String)>;

    fn origin_methods(&self) -> &'static [OriginMethod];
    async fn verify_key_origin(
        &self,
        ep: &Endpoint,
        method: OriginMethod,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<KeyOrigin>;
    fn origin_from_response(&self, ep: &Endpoint, headers: &HeaderMap) -> Option<KeyOrigin>;
    fn model_from_request(
        &self,
        ep: &Endpoint,
        method: &Method,
        path: &str,
        body_prefix: &[u8],
        truncated: bool,
    ) -> ModelRef {
        let _ = ep;
        crate::model_from_request(method, path, body_prefix, truncated)
    }
}
