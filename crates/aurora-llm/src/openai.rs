use crate::http_util::{check, host_of, json, send};
use crate::{
    AuthMode, Dialect, Endpoint, EndpointKind, InjectionHeader, KeyOrigin, LlmError, LlmInjection,
    LlmProvider, ModelInfo, OriginMethod, Result,
};
use aurora_core::Secret;
use http::{HeaderMap, Method};

pub const OPENAI_BASE_URL: &str = "https://api.openai.com";
const PLACEHOLDER: &str = "aurora-placeholder";

/// OpenAI (`08 §Phase 4 providers`, C-800, C-806, C-809): fixed `https://api.openai.com` (replaceable only through
/// [`OpenaiProvider::with_base_url`], i.e. `llm.dev_upstream_overrides`); the key goes in `Authorization: Bearer`,
/// the entry's `organization` / `project` become `OpenAI-Organization` / `OpenAI-Project`.
///
/// Pinned calls: models `GET <base>/v1/models`, whose response headers `openai-organization` / `openai-project`
/// are the key's account (the `provider_account` origin and the response backstop).
#[derive(Debug, Clone)]
pub struct OpenaiProvider {
    base: String,
}

impl Default for OpenaiProvider {
    fn default() -> Self {
        Self {
            base: OPENAI_BASE_URL.into(),
        }
    }
}

impl OpenaiProvider {
    /// For `llm.dev_upstream_overrides` and tests; production uses the fixed URL.
    pub fn with_base_url(base: impl Into<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_string(),
        }
    }

    fn request(
        &self,
        ep: &Endpoint,
        http: &reqwest::Client,
        key: &Secret<String>,
    ) -> reqwest::RequestBuilder {
        let mut r = http
            .get(format!("{}/v1/models", self.base))
            .bearer_auth(key.expose());
        if let Some(o) = &ep.organization {
            r = r.header("OpenAI-Organization", o);
        }
        if let Some(p) = &ep.project {
            r = r.header("OpenAI-Project", p);
        }
        r
    }

    async fn fetch(
        &self,
        ep: &Endpoint,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<(Vec<ModelInfo>, HeaderMap)> {
        let resp = check(send(self.request(ep, http, key)).await?)?;
        let headers = resp.headers().clone();
        let v = json(resp).await?;
        let data = v
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| LlmError::BadResponse("missing `data` array".into()))?;
        let models = data
            .iter()
            .filter_map(|m| m.get("id").and_then(|i| i.as_str()))
            .map(|id| ModelInfo {
                id: id.to_string(),
                display_name: None,
            })
            .collect();
        Ok((models, headers))
    }

    pub(crate) fn origin_of(headers: &HeaderMap) -> Option<KeyOrigin> {
        let get = |n: &str| {
            headers
                .get(n)
                .and_then(|v| v.to_str().ok())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let organization_id = get("openai-organization");
        let workspace_id = get("openai-project");
        if organization_id.is_none() && workspace_id.is_none() {
            return None;
        }
        Some(KeyOrigin {
            organization_id,
            workspace_id,
            ..KeyOrigin::default()
        })
    }
}

#[async_trait::async_trait]
impl LlmProvider for OpenaiProvider {
    fn id(&self) -> &'static str {
        "openai"
    }
    fn display_name(&self) -> &'static str {
        "OpenAI"
    }
    fn endpoint_kind(&self) -> EndpointKind {
        EndpointKind::Fixed(OPENAI_BASE_URL)
    }
    fn auth_modes(&self) -> &'static [AuthMode] {
        &[AuthMode::ApiKey]
    }
    fn dialects(&self, ep: &Endpoint) -> Vec<Dialect> {
        if ep.dialects.is_empty() {
            vec![Dialect::OpenaiChat, Dialect::OpenaiResponses]
        } else {
            ep.dialects.clone()
        }
    }
    fn hosts(&self, _ep: &Endpoint) -> Vec<String> {
        host_of(&self.base).into_iter().collect()
    }
    fn base_url(&self, _ep: &Endpoint) -> String {
        self.base.clone()
    }
    fn injection(&self, _mode: &AuthMode) -> LlmInjection {
        LlmInjection::new(
            InjectionHeader::Bearer,
            &[
                "authorization",
                "x-api-key",
                "api-key",
                "openai-organization",
                "openai-project",
            ],
        )
    }
    fn injection_for(&self, ep: &Endpoint, mode: &AuthMode) -> LlmInjection {
        let mut i = self.injection(mode);
        if let Some(o) = &ep.organization {
            i.set_headers.push(("openai-organization", o.clone()));
        }
        if let Some(p) = &ep.project {
            i.set_headers.push(("openai-project", p.clone()));
        }
        i
    }
    fn inference_path_permitted(&self, ep: &Endpoint, method: &Method, path: &str) -> bool {
        crate::paths::openai_path_permitted(method, path, &ep.allowed_paths)
    }
    async fn list_models(
        &self,
        ep: &Endpoint,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<Vec<ModelInfo>> {
        match self.fetch(ep, key, http).await {
            Ok((m, _)) => Ok(m),
            Err(e) if !matches!(e, LlmError::InvalidKey) && !ep.models.is_empty() => Ok(ep
                .models
                .iter()
                .map(|id| ModelInfo {
                    id: id.clone(),
                    display_name: None,
                })
                .collect()),
            Err(e) => Err(e),
        }
    }
    async fn list_models_strict(
        &self,
        ep: &Endpoint,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<Vec<ModelInfo>> {
        self.fetch(ep, key, http).await.map(|(m, _)| m)
    }
    fn default_model(&self, ep: &Endpoint) -> Option<String> {
        ep.default_model.clone()
    }
    fn client_env(
        &self,
        _ep: &Endpoint,
        _dialect: Dialect,
        _mode: &AuthMode,
    ) -> Vec<(String, String)> {
        vec![
            ("OPENAI_BASE_URL".into(), format!("{}/v1", self.base)),
            ("OPENAI_API_KEY".into(), PLACEHOLDER.into()),
        ]
    }

    fn origin_methods(&self) -> &'static [OriginMethod] {
        &[OriginMethod::ProviderAccount]
    }
    async fn verify_key_origin(
        &self,
        ep: &Endpoint,
        method: OriginMethod,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<KeyOrigin> {
        match method {
            OriginMethod::ProviderAccount => {
                let (_, headers) = self.fetch(ep, key, http).await?;
                Self::origin_of(&headers)
                    .ok_or_else(|| LlmError::Unverifiable("no_organization_header".into()))
            }
            other => Err(LlmError::UnsupportedMethod(other.as_str())),
        }
    }
    fn origin_from_response(&self, _ep: &Endpoint, headers: &HeaderMap) -> Option<KeyOrigin> {
        Self::origin_of(headers)
    }
}
