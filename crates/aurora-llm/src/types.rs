use crate::{Endpoint, LlmError, Result};
use aurora_core::Secret;
use http::{HeaderMap, Method};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dialect {
    AnthropicMessages,
    OpenaiChat,
    OpenaiResponses,
}

impl Dialect {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "anthropic_messages" => Some(Self::AnthropicMessages),
            "openai_chat" => Some(Self::OpenaiChat),
            "openai_responses" => Some(Self::OpenaiResponses),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AnthropicMessages => "anthropic_messages",
            Self::OpenaiChat => "openai_chat",
            Self::OpenaiResponses => "openai_responses",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    ApiKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointKind {
    Fixed(&'static str),
    Configured,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmInjection {
    pub header: InjectionHeader,
    /// Agent-supplied request headers to remove (lowercase).
    pub strip_request_headers: Vec<&'static str>,
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
    async fn list_models(
        &self,
        ep: &Endpoint,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<Vec<ModelInfo>>;
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
