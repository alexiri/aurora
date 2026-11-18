use crate::http_util::{check, host_of, json, send};
use crate::{
    AuthMode, Dialect, Endpoint, EndpointKind, InjectionHeader, KeyOrigin, LlmError, LlmInjection,
    LlmProvider, ModelInfo, OriginMethod, Result,
};
use aurora_core::Secret;
use http::HeaderMap;

const PLACEHOLDER: &str = "aurora-placeholder";
const DEFAULT_DIALECTS: [Dialect; 3] = [
    Dialect::OpenaiChat,
    Dialect::OpenaiResponses,
    Dialect::AnthropicMessages,
];

/// LiteLLM proxy (operator-configured base URL).
///
/// Pinned calls: models `GET <base>/v1/models`; team origin `GET <base>/key/info` called
/// with the key as `Authorization: Bearer`, reading `info.team_id` / `info.user_id`.
#[derive(Debug, Default, Clone, Copy)]
pub struct LitellmProvider;

impl LitellmProvider {
    async fn fetch_models(
        ep: &Endpoint,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<Vec<ModelInfo>> {
        let url = format!("{}/v1/models", ep.base());
        let resp = check(send(http.get(url).bearer_auth(key.expose())).await?)?;
        let v = json(resp).await?;
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
impl LlmProvider for LitellmProvider {
    fn id(&self) -> &'static str {
        "litellm"
    }
    fn display_name(&self) -> &'static str {
        "LiteLLM"
    }
    fn endpoint_kind(&self) -> EndpointKind {
        EndpointKind::Configured
    }
    fn auth_modes(&self) -> &'static [AuthMode] {
        &[AuthMode::ApiKey]
    }
    fn dialects(&self, ep: &Endpoint) -> Vec<Dialect> {
        if ep.dialects.is_empty() {
            DEFAULT_DIALECTS.to_vec()
        } else {
            ep.dialects.clone()
        }
    }
    fn hosts(&self, ep: &Endpoint) -> Vec<String> {
        host_of(&ep.base_url).into_iter().collect()
    }
    fn injection(&self, _mode: &AuthMode) -> LlmInjection {
        LlmInjection::new(InjectionHeader::Bearer, &["x-api-key"])
    }
    /// (D-177, SEC-65, D-367) The `litellm` inference set (`inference_path_permitted`), plus the one
    /// read-only metadata route goose's `litellm` provider calls at every start: `GET <base
    /// path>/model/info` (the model's context window). That path and method only: every other
    /// metadata or management route stays refused, and `allowed_paths` still only adds.
    fn inference_path_permitted(&self, ep: &Endpoint, method: &http::Method, path: &str) -> bool {
        crate::inference_path_permitted(method, path, &ep.allowed_paths)
            || (*method == http::Method::GET
                && crate::paths::allowed_path_match(
                    path,
                    &[format!("{}/model/info", ep.base_path())],
                ))
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
                ("ANTHROPIC_API_KEY".into(), PLACEHOLDER.into()),
            ],
            // (a LiteLLM endpoint never declares the cloud dialects, config refuses them; OpenAI form is the fallback)
            Dialect::OpenaiChat
            | Dialect::OpenaiResponses
            | Dialect::AnthropicBedrock
            | Dialect::AnthropicVertex => vec![
                ("OPENAI_BASE_URL".into(), format!("{base}/v1")),
                ("OPENAI_API_KEY".into(), PLACEHOLDER.into()),
            ],
        }
    }

    fn origin_methods(&self) -> &'static [OriginMethod] {
        &[OriginMethod::Endpoint, OriginMethod::LitellmTeam]
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
            OriginMethod::LitellmTeam => {
                let url = format!("{}/key/info", ep.base());
                let resp = send(http.get(url).bearer_auth(key.expose())).await?;
                // The key already passed `/v1/models`; a refusal here is a `/key/info`
                // problem (e.g. the proxy restricts the route), not an invalid key. Keep
                // the status so the user sees which step failed (C-502).
                let status = resp.status();
                if status == http::StatusCode::UNAUTHORIZED || status == http::StatusCode::FORBIDDEN
                {
                    return Err(LlmError::Unverifiable(format!(
                        "key_info_http_{}",
                        status.as_u16()
                    )));
                }
                // Other failures stay `Status` (the gateway treats them as transient).
                let resp = check(resp)?;
                let v = json(resp)
                    .await
                    .map_err(|_| LlmError::Unverifiable("bad_response".into()))?;
                let field = |n: &str| {
                    v.pointer(&format!("/info/{n}"))
                        .and_then(|x| x.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                };
                // A key without a team is reported as such (`team_id = None`); whether that
                // is acceptable depends on the rule, so the callers decide
                // (`origin::requires_team`, 08 §11.3).
                Ok(KeyOrigin {
                    team_id: field("team_id"),
                    user_id: field("user_id"),
                    ..KeyOrigin::default()
                })
            }
            OriginMethod::ProviderAccount => Err(LlmError::UnsupportedMethod(
                OriginMethod::ProviderAccount.as_str(),
            )),
        }
    }
    fn origin_from_response(&self, _ep: &Endpoint, _headers: &HeaderMap) -> Option<KeyOrigin> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Method;

    fn ep(base: &str, allowed: &[&str]) -> Endpoint {
        Endpoint {
            id: "central".into(),
            provider: "litellm".into(),
            display_name: String::new(),
            base_url: base.into(),
            dialects: vec![],
            default_model: None,
            models: vec![],
            allowed_models: vec![],
            key_origin: None,
            allowed_paths: allowed.iter().map(|s| (*s).to_owned()).collect(),
            organization: None,
            project: None,
            auth_header: None,
            models_path: None,
            region: None,
            project_id: None,
            location: None,
            model_roles: Default::default(),
            model_info: Default::default(),
        }
    }

    fn ok(e: &Endpoint, m: &Method, p: &str) -> bool {
        LitellmProvider.inference_path_permitted(e, m, p)
    }

    /// (D-367) `GET /model/info` is part of the `litellm` default set, with no `allowed_paths`;
    /// that path and method only.
    #[test]
    fn get_model_info_is_in_the_default_set_and_only_that_route() {
        let e = ep("https://llm.example", &[]);
        assert!(ok(&e, &Method::GET, "/model/info"));
        assert!(ok(&e, &Method::GET, "/model/info?litellm_model_id=x"));
        assert!(ok(&e, &Method::GET, "/model/info/"));
        assert!(!ok(&e, &Method::POST, "/model/info"), "POST is refused");
        assert!(!ok(&e, &Method::DELETE, "/model/info"));
        for p in [
            "/model/info/extra",
            "/model/group/info",
            "/model/new",
            "/model/delete",
            "/v1/model/info",
            "/model_group/info",
            "/key/info",
            "/model/./info",
            "/model/%2e%2e/info",
            "/other/model/info",
        ] {
            assert!(!ok(&e, &Method::GET, p), "GET {p}");
        }
        // The inference set is unchanged.
        assert!(ok(&e, &Method::POST, "/v1/chat/completions"));
        assert!(ok(&e, &Method::GET, "/v1/models"));
        assert!(!ok(&e, &Method::GET, "/key/generate"));
    }

    /// A deployment under a route prefix reaches its own `…/model/info` only.
    #[test]
    fn under_a_base_path_only_that_prefix_counts() {
        let e = ep("https://llm.example/litellm/", &[]);
        assert!(ok(&e, &Method::GET, "/litellm/model/info"));
        assert!(!ok(&e, &Method::GET, "/model/info"));
        assert!(!ok(&e, &Method::GET, "/other/model/info"));
    }

    /// `allowed_paths` still only adds: other metadata routes are admitted when listed.
    #[test]
    fn allowed_paths_still_add() {
        let e = ep("https://llm.example", &["/model/group/info"]);
        assert!(ok(&e, &Method::GET, "/model/group/info"));
        assert!(ok(&e, &Method::GET, "/model/info"));
        assert!(!ok(&e, &Method::GET, "/model/new"));
    }
}
