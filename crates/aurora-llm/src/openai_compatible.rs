use crate::http_util::{check, host_of, json, send};
use crate::{
    AuthMode, Dialect, Endpoint, EndpointKind, InjectionHeader, KeyOrigin, LlmError, LlmInjection,
    LlmProvider, ModelInfo, OriginMethod, Result,
};
use aurora_core::Secret;
use http::{HeaderMap, Method};

const PLACEHOLDER: &str = "aurora-placeholder";

/// Any OpenAI-style server (`08 §Phase 4 providers`, C-800, C-806, C-809): a configured base URL with an optional path
/// prefix, the key in the entry's `auth_header` (`authorization` = Bearer, `api-key`, `x-api-key`), models from
/// `models_path` (default `/v1/models`), key origin `endpoint` only.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenaiCompatibleProvider;

impl OpenaiCompatibleProvider {
    fn header(ep: &Endpoint) -> InjectionHeader {
        match ep.auth_header.as_deref() {
            Some("api-key") => InjectionHeader::Header("api-key"),
            Some("x-api-key") => InjectionHeader::XApiKey,
            _ => InjectionHeader::Bearer,
        }
    }

    async fn fetch_models(
        ep: &Endpoint,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<Vec<ModelInfo>> {
        let url = format!(
            "{}{}",
            ep.base(),
            ep.models_path.as_deref().unwrap_or("/v1/models")
        );
        let rb = http.get(url);
        let rb = match Self::header(ep) {
            InjectionHeader::XApiKey => rb.header("x-api-key", key.expose()),
            InjectionHeader::Header(n) => rb.header(n, key.expose()),
            _ => rb.bearer_auth(key.expose()),
        };
        let v = json(check(send(rb).await?)?).await?;
        let data = v
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| LlmError::BadResponse("missing `data` array".into()))?;
        Ok(data
            .iter()
            .filter_map(|m| m.get("id").and_then(|i| i.as_str()))
            .map(|id| ModelInfo {
                id: id.to_string(),
                display_name: None,
            })
            .collect())
    }
}

#[async_trait::async_trait]
impl LlmProvider for OpenaiCompatibleProvider {
    fn id(&self) -> &'static str {
        "openai_compatible"
    }
    fn display_name(&self) -> &'static str {
        "OpenAI-compatible"
    }
    fn endpoint_kind(&self) -> EndpointKind {
        EndpointKind::Configured
    }
    fn auth_modes(&self) -> &'static [AuthMode] {
        &[AuthMode::ApiKey]
    }
    fn dialects(&self, ep: &Endpoint) -> Vec<Dialect> {
        if ep.dialects.is_empty() {
            vec![Dialect::OpenaiChat]
        } else {
            ep.dialects.clone()
        }
    }
    fn hosts(&self, ep: &Endpoint) -> Vec<String> {
        host_of(&ep.base_url).into_iter().collect()
    }
    fn injection(&self, _mode: &AuthMode) -> LlmInjection {
        LlmInjection::new(
            InjectionHeader::Bearer,
            &["authorization", "x-api-key", "api-key"],
        )
    }
    fn injection_for(&self, ep: &Endpoint, mode: &AuthMode) -> LlmInjection {
        let mut i = self.injection(mode);
        i.header = Self::header(ep);
        i
    }
    fn inference_path_permitted(&self, ep: &Endpoint, method: &Method, path: &str) -> bool {
        crate::paths::compatible_path_permitted(
            method,
            path,
            &ep.base_path(),
            &self.dialects(ep),
            ep.models_path.as_deref(),
            &ep.allowed_paths,
        )
    }
    async fn list_models(
        &self,
        ep: &Endpoint,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<Vec<ModelInfo>> {
        match Self::fetch_models(ep, key, http).await {
            Err(e) if !matches!(e, LlmError::InvalidKey) && !ep.models.is_empty() => Ok(ep
                .models
                .iter()
                .map(|id| ModelInfo {
                    id: id.clone(),
                    display_name: None,
                })
                .collect()),
            other => other,
        }
    }
    async fn list_models_strict(
        &self,
        ep: &Endpoint,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<Vec<ModelInfo>> {
        Self::fetch_models(ep, key, http).await
    }
    fn default_model(&self, ep: &Endpoint) -> Option<String> {
        ep.default_model.clone()
    }
    fn client_env(
        &self,
        ep: &Endpoint,
        dialect: Dialect,
        _mode: &AuthMode,
    ) -> Vec<(String, String)> {
        let base = ep.base();
        match dialect {
            Dialect::AnthropicMessages => vec![
                ("ANTHROPIC_BASE_URL".into(), base.into()),
                ("ANTHROPIC_AUTH_TOKEN".into(), PLACEHOLDER.into()),
            ],
            _ => vec![
                ("OPENAI_BASE_URL".into(), format!("{base}/v1")),
                ("OPENAI_API_KEY".into(), PLACEHOLDER.into()),
            ],
        }
    }

    fn origin_methods(&self) -> &'static [OriginMethod] {
        &[OriginMethod::Endpoint]
    }
    async fn verify_key_origin(
        &self,
        ep: &Endpoint,
        method: OriginMethod,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<KeyOrigin> {
        match method {
            OriginMethod::Endpoint => {
                Self::fetch_models(ep, key, http).await?;
                Ok(KeyOrigin::default())
            }
            other => Err(LlmError::UnsupportedMethod(other.as_str())),
        }
    }
    fn origin_from_response(&self, _ep: &Endpoint, _headers: &HeaderMap) -> Option<KeyOrigin> {
        None
    }
}
