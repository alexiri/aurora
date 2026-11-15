use crate::http_util::host_of;
use crate::mint::{self, MintClass, MintCtx};
use crate::{
    AuthMode, Dialect, Endpoint, EndpointKind, InjectionHeader, KeyOrigin, LlmError, LlmInjection,
    LlmProvider, ModelInfo, ModelRef, OriginMethod, Overrides, Result,
};
use aurora_core::Secret;
use http::{HeaderMap, Method};
use std::time::SystemTime;

const PLACEHOLDER: &str = "aurora-placeholder";

/// Google Vertex AI (`08 §Phase 4 providers`, C-800, C-804, C-806, C-809, C-810): the host follows the entry's
/// `location` (`<location>-aiplatform.googleapis.com`, `aiplatform.googleapis.com` for `global`); the key is a
/// service-account file the gateway turns into short-lived access tokens (`mint::gcp`); inference only on prediction
/// routes of the entry's own project and location; models are the entry's static list.
#[derive(Debug, Clone, Default)]
pub struct VertexProvider {
    overrides: Overrides,
}

impl VertexProvider {
    /// With `llm.dev_upstream_overrides` applied to the Vertex origins (tests and development only).
    #[must_use]
    pub fn with_overrides(overrides: Overrides) -> Self {
        Self { overrides }
    }

    /// The pinned origin of an entry: `https://<location>-aiplatform.googleapis.com`.
    #[must_use]
    pub fn pinned_origin(ep: &Endpoint) -> String {
        match ep.location.as_deref() {
            Some("global") | None => "https://aiplatform.googleapis.com".to_owned(),
            Some(l) => format!("https://{l}-aiplatform.googleapis.com"),
        }
    }

    fn ctx<'a>(&'a self, ep: &'a Endpoint, value: &'a [u8]) -> MintCtx<'a> {
        MintCtx {
            endpoint: ep,
            value,
            org_id: "",
            org_external_id: None,
            now: SystemTime::now(),
            overrides: &self.overrides,
        }
    }

    /// One mint with the key; the evidence of both validation and origin.
    async fn mint(
        &self,
        ep: &Endpoint,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<KeyOrigin> {
        let value = key.expose().as_bytes();
        mint::gcp::parse_value(value)
            .map_err(|e| LlmError::Unverifiable(format!("key_invalid:{}", e.reason)))?;
        let ctx = self.ctx(ep, value);
        match mint::run(http, &mint::gcp::GcpSpec, &ctx).await {
            Ok(m) => m
                .origin
                .ok_or_else(|| LlmError::Unverifiable("no_project".into())),
            Err(e) if e.class == MintClass::Rejected => Err(LlmError::InvalidKey),
            Err(_) => Err(LlmError::Unreachable("token endpoint unavailable".into())),
        }
    }
}

/// `…/models/<model>:<op>` of a prediction route.
fn path_model(path: &str) -> Option<&str> {
    let last = path.trim_end_matches('/').rsplit('/').next()?;
    let (model, _) = last.split_once(':')?;
    path.contains("/models/").then_some(model)
}

#[async_trait::async_trait]
impl LlmProvider for VertexProvider {
    fn id(&self) -> &'static str {
        "vertex"
    }
    fn display_name(&self) -> &'static str {
        "Google Vertex AI"
    }
    fn endpoint_kind(&self) -> EndpointKind {
        EndpointKind::Derived
    }
    fn auth_modes(&self) -> &'static [AuthMode] {
        &[AuthMode::GcpServiceAccount]
    }
    fn dialects(&self, ep: &Endpoint) -> Vec<Dialect> {
        if ep.dialects.is_empty() {
            vec![Dialect::AnthropicVertex]
        } else {
            ep.dialects.clone()
        }
    }
    fn hosts(&self, ep: &Endpoint) -> Vec<String> {
        host_of(&self.base_url(ep)).into_iter().collect()
    }
    fn base_url(&self, ep: &Endpoint) -> String {
        self.overrides.origin(&Self::pinned_origin(ep))
    }
    fn injection(&self, _mode: &AuthMode) -> LlmInjection {
        let mut i = LlmInjection::new(
            InjectionHeader::GcpToken,
            &["authorization", "x-api-key", "api-key"],
        );
        i.strip_request_prefixes.push("x-goog-");
        i
    }
    fn inference_path_permitted(&self, ep: &Endpoint, method: &Method, path: &str) -> bool {
        // No `allowed_paths`, ever (C-800 refuses them on `vertex`).
        crate::paths::vertex_path_permitted(
            method,
            path,
            ep.project_id.as_deref().unwrap_or_default(),
            ep.location.as_deref().unwrap_or_default(),
            self.dialects(ep).contains(&Dialect::OpenaiChat),
        )
    }
    fn model_from_request(
        &self,
        _ep: &Endpoint,
        method: &Method,
        path: &str,
        body_prefix: &[u8],
        truncated: bool,
    ) -> ModelRef {
        // The prediction routes carry the model in the path; the OpenAI-compatible route in the body.
        if let Some(m) = path_model(path) {
            return ModelRef::Model(m.to_owned());
        }
        if *method != Method::POST || !path.trim_end_matches('/').ends_with("/chat/completions") {
            return ModelRef::NoModel;
        }
        if truncated {
            return ModelRef::Unknown;
        }
        match serde_json::from_slice::<serde_json::Value>(body_prefix)
            .ok()
            .as_ref()
            .and_then(|v| v.pointer("/model"))
            .and_then(|m| m.as_str())
        {
            Some(m) if !m.is_empty() => ModelRef::Model(m.to_owned()),
            _ => ModelRef::Unknown,
        }
    }
    async fn list_models(
        &self,
        ep: &Endpoint,
        _key: &Secret<String>,
        _http: &reqwest::Client,
    ) -> Result<Vec<ModelInfo>> {
        Ok(static_models(ep))
    }
    /// The key is validated by minting a token (no listing call that needs no extra IAM).
    async fn list_models_strict(
        &self,
        ep: &Endpoint,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<Vec<ModelInfo>> {
        match self.mint(ep, key, http).await {
            Ok(_) => Ok(static_models(ep)),
            Err(LlmError::Unverifiable(_)) => Err(LlmError::InvalidKey),
            Err(e) => Err(e),
        }
    }
    fn validates_by_origin(&self) -> bool {
        true
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
        let base = self.base_url(ep);
        let project = ep.project_id.clone().unwrap_or_default();
        let location = ep.location.clone().unwrap_or_default();
        match dialect {
            Dialect::OpenaiChat | Dialect::OpenaiResponses => vec![
                (
                    "OPENAI_BASE_URL".into(),
                    format!("{base}/v1/projects/{project}/locations/{location}/endpoints/openapi"),
                ),
                ("OPENAI_API_KEY".into(), PLACEHOLDER.into()),
            ],
            _ => vec![
                ("CLAUDE_CODE_USE_VERTEX".into(), "1".into()),
                ("CLOUD_ML_REGION".into(), location),
                ("ANTHROPIC_VERTEX_PROJECT_ID".into(), project),
                ("ANTHROPIC_VERTEX_BASE_URL".into(), format!("{base}/v1")),
                ("CLAUDE_CODE_SKIP_VERTEX_AUTH".into(), "1".into()),
            ],
        }
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
            OriginMethod::ProviderAccount => match self.mint(ep, key, http).await {
                Err(LlmError::Unverifiable(_)) => Err(LlmError::InvalidKey),
                other => other,
            },
            other => Err(LlmError::UnsupportedMethod(other.as_str())),
        }
    }
    fn origin_from_response(&self, _ep: &Endpoint, _headers: &HeaderMap) -> Option<KeyOrigin> {
        None
    }
}

fn static_models(ep: &Endpoint) -> Vec<ModelInfo> {
    ep.models
        .iter()
        .map(|id| ModelInfo {
            id: id.clone(),
            display_name: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(location: &str) -> Endpoint {
        Endpoint {
            id: "v".into(),
            provider: "vertex".into(),
            display_name: "Vertex".into(),
            base_url: String::new(),
            dialects: vec![],
            default_model: None,
            models: vec!["claude-x".into()],
            allowed_models: vec![],
            key_origin: None,
            allowed_paths: vec![],
            organization: None,
            project: None,
            auth_header: None,
            models_path: None,
            region: None,
            project_id: Some("proj-1".into()),
            location: Some(location.into()),
        }
    }

    #[test]
    fn host_follows_location() {
        let p = VertexProvider::default();
        assert_eq!(
            p.hosts(&ep("us-east5")),
            ["us-east5-aiplatform.googleapis.com"]
        );
        assert_eq!(p.hosts(&ep("global")), ["aiplatform.googleapis.com"]);
    }

    #[test]
    fn override_replaces_the_origin() {
        let o = Overrides::new(
            &[(
                "https://us-east5-aiplatform.googleapis.com".to_owned(),
                "http://127.0.0.1:9".to_owned(),
            )]
            .into(),
        );
        let p = VertexProvider::with_overrides(o);
        assert_eq!(p.base_url(&ep("us-east5")), "http://127.0.0.1:9");
        assert_eq!(p.hosts(&ep("us-east5")), ["127.0.0.1:9"]);
    }

    #[test]
    fn model_from_the_path_or_the_body() {
        let p = VertexProvider::default();
        let e = ep("us-east5");
        let route = "/v1/projects/proj-1/locations/us-east5/publishers/anthropic/models/claude-x@1:rawPredict";
        assert_eq!(
            p.model_from_request(&e, &Method::POST, route, b"{}", false),
            ModelRef::Model("claude-x@1".into())
        );
        assert_eq!(
            p.model_from_request(
                &e,
                &Method::POST,
                "/v1/projects/proj-1/locations/us-east5/endpoints/openapi/chat/completions",
                br#"{"model":"publishers/anthropic/models/claude-y"}"#,
                false
            ),
            ModelRef::Model("publishers/anthropic/models/claude-y".into())
        );
    }

    #[test]
    fn client_env_has_no_credential() {
        let p = VertexProvider::default();
        let env = p.client_env(
            &ep("us-east5"),
            Dialect::AnthropicVertex,
            &AuthMode::GcpServiceAccount,
        );
        assert!(
            env.iter()
                .any(|(k, v)| k == "CLOUD_ML_REGION" && v == "us-east5")
        );
        assert!(
            env.iter()
                .all(|(_, v)| !v.contains("BEGIN") && !v.starts_with("ya29"))
        );
    }
}
