use crate::http_util::{check, host_of, json, send};
use crate::{
    AuthMode, Dialect, Endpoint, EndpointKind, InjectionHeader, KeyOrigin, LlmError, LlmInjection,
    LlmProvider, ModelInfo, OriginMethod, Result,
};
use aurora_core::Secret;
use http::HeaderMap;

pub const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
const VERSION: &str = "2023-06-01";
const MAX_PAGES: usize = 50;

/// Anthropic API; fixed base URL (overridable only through [`AnthropicProvider::with_base_url`]).
#[derive(Debug, Clone)]
pub struct AnthropicProvider {
    base: String,
}

impl Default for AnthropicProvider {
    fn default() -> Self {
        Self {
            base: ANTHROPIC_BASE_URL.into(),
        }
    }
}

impl AnthropicProvider {
    /// For tests only; production uses the fixed URL and config cannot override it.
    pub fn with_base_url(base: impl Into<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_string(),
        }
    }

    fn get(
        &self,
        http: &reqwest::Client,
        path: &str,
        key: &Secret<String>,
    ) -> reqwest::RequestBuilder {
        http.get(format!("{}{path}", self.base))
            .header("x-api-key", key.expose())
            .header("anthropic-version", VERSION)
    }

    async fn fetch_models(
        &self,
        http: &reqwest::Client,
        key: &Secret<String>,
    ) -> Result<Vec<ModelInfo>> {
        let mut out = Vec::new();
        let mut after: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut req = self
                .get(http, "/v1/models", key)
                .query(&[("limit", "1000")]);
            if let Some(a) = &after {
                req = req.query(&[("after_id", a.as_str())]);
            }
            let v = json(check(send(req).await?)?).await?;
            let data = v
                .get("data")
                .and_then(|d| d.as_array())
                .ok_or_else(|| LlmError::BadResponse("missing `data` array".into()))?;
            out.extend(data.iter().filter_map(|m| {
                Some(ModelInfo {
                    id: m.get("id")?.as_str()?.to_string(),
                    display_name: m
                        .get("display_name")
                        .and_then(|d| d.as_str())
                        .map(str::to_string),
                })
            }));
            let more = v.get("has_more").and_then(|b| b.as_bool()).unwrap_or(false);
            match v.get("last_id").and_then(|l| l.as_str()) {
                Some(l) if more => after = Some(l.to_string()),
                _ => return Ok(out),
            }
        }
        Ok(out)
    }

    fn origin_of(headers: &HeaderMap) -> Option<KeyOrigin> {
        let get = |n: &str| {
            headers
                .get(n)
                .and_then(|v| v.to_str().ok())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let organization_id = get("anthropic-organization-id");
        let workspace_id = get("anthropic-workspace-id");
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
impl LlmProvider for AnthropicProvider {
    fn id(&self) -> &'static str {
        "anthropic"
    }
    fn display_name(&self) -> &'static str {
        "Anthropic"
    }
    fn endpoint_kind(&self) -> EndpointKind {
        EndpointKind::Fixed(ANTHROPIC_BASE_URL)
    }
    fn auth_modes(&self) -> &'static [AuthMode] {
        &[AuthMode::ApiKey]
    }
    fn dialects(&self, _ep: &Endpoint) -> Vec<Dialect> {
        vec![Dialect::AnthropicMessages]
    }
    fn hosts(&self, _ep: &Endpoint) -> Vec<String> {
        host_of(&self.base).into_iter().collect()
    }
    fn injection(&self, _mode: &AuthMode) -> LlmInjection {
        LlmInjection {
            header: InjectionHeader::XApiKey,
            strip_request_headers: vec!["authorization"],
        }
    }
    async fn list_models(
        &self,
        ep: &Endpoint,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<Vec<ModelInfo>> {
        match self.fetch_models(http, key).await {
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
            ("ANTHROPIC_BASE_URL".into(), self.base.clone()),
            ("ANTHROPIC_API_KEY".into(), "aurora-placeholder".into()),
        ]
    }

    fn origin_methods(&self) -> &'static [OriginMethod] {
        &[OriginMethod::ProviderAccount]
    }
    async fn verify_key_origin(
        &self,
        _ep: &Endpoint,
        method: OriginMethod,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<KeyOrigin> {
        if method != OriginMethod::ProviderAccount {
            return Err(LlmError::UnsupportedMethod(method.as_str()));
        }
        let req = self.get(http, "/v1/models", key).query(&[("limit", "1")]);
        let resp = check(send(req).await?)?;
        Self::origin_of(resp.headers())
            .ok_or_else(|| LlmError::Unverifiable("provider reported no account headers".into()))
    }
    fn origin_from_response(&self, _ep: &Endpoint, headers: &HeaderMap) -> Option<KeyOrigin> {
        Self::origin_of(headers)
    }
}
