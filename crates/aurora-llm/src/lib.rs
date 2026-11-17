//! aurora-llm — core LLM provider registry (docs/08-secrets-credentials.md §6, §11.3).
//!
//! Providers are a closed core registry, not plugins. Host patterns are returned as
//! strings in policy host-pattern syntax (`host` or `host:port`); callers convert them.
#![forbid(unsafe_code)]

mod anthropic;
mod azure_openai;
mod bedrock;
mod endpoint;
mod error;
pub mod governance;
mod litellm;
pub mod mint;
mod model;
mod openai;
mod openai_compatible;
pub mod origin;
mod overrides;
pub mod paths;
mod registry;
pub mod sigv4;
mod types;
mod vertex;

pub use anthropic::{ANTHROPIC_BASE_URL, AnthropicProvider};
pub use azure_openai::AzureOpenaiProvider;
pub use bedrock::BedrockProvider;
// (P4-LLM-5) The table lives in `aurora-core` so the sandbox's supervisor shares it without the provider stack.
pub use aurora_core::llm_classify::{LlmErrorClass, classify};
// (P5-MODELS-1, SEC-122) The one model-id validator; it lives in `aurora-core` so the configuration crate and the
// sandbox's supervisor share it without the provider stack.
pub use aurora_core::model_id::{MODEL_ID_PATTERN, validate_model_id};
pub use endpoint::Endpoint;
pub use error::{LlmError, Result};
pub use litellm::LitellmProvider;
pub use model::{inference_path_permitted, model_from_request};
pub use openai::{OPENAI_BASE_URL, OpenaiProvider};
pub use openai_compatible::OpenaiCompatibleProvider;
pub use overrides::Overrides;
pub use registry::{
    ProviderRegistry, endpoint_auth_modes, governed_hosts, recommended_auth_mode,
    supported_providers,
};
pub use types::{
    AuthMode, Dialect, EndpointKind, InjectionHeader, KeyCheck, KeyOrigin, KeyOriginRule,
    LlmInjection, LlmProvider, ModelInfo, ModelRef, OriginMethod,
};
pub use vertex::VertexProvider;

pub(crate) mod http_util {
    use crate::LlmError;

    /// Send a request, mapping transport failures to a key-free `LlmError`.
    pub(crate) async fn send(req: reqwest::RequestBuilder) -> Result<reqwest::Response, LlmError> {
        req.send().await.map_err(|e| {
            LlmError::Unreachable(
                if e.is_timeout() {
                    "request timed out"
                } else if e.is_connect() {
                    "connection failed"
                } else {
                    "request failed"
                }
                .to_string(),
            )
        })
    }

    /// 401/403 => InvalidKey, other non-2xx => Status.
    pub(crate) fn check(resp: reqwest::Response) -> Result<reqwest::Response, LlmError> {
        let s = resp.status();
        if s == reqwest::StatusCode::UNAUTHORIZED || s == reqwest::StatusCode::FORBIDDEN {
            Err(LlmError::InvalidKey)
        } else if !s.is_success() {
            Err(LlmError::Status(s.as_u16()))
        } else {
            Ok(resp)
        }
    }

    pub(crate) async fn json(resp: reqwest::Response) -> Result<serde_json::Value, LlmError> {
        resp.json()
            .await
            .map_err(|_| LlmError::BadResponse("response body is not valid JSON".into()))
    }

    /// Host (lowercased) with `:port` when explicit.
    pub(crate) fn host_of(base_url: &str) -> Option<String> {
        let uri: http::Uri = base_url.parse().ok()?;
        let host = uri.host()?.to_ascii_lowercase();
        Some(match uri.port_u16() {
            Some(p) => format!("{host}:{p}"),
            None => host,
        })
    }
}
