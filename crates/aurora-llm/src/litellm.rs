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
        LlmInjection {
            header: InjectionHeader::Bearer,
            strip_request_headers: vec!["x-api-key"],
        }
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
            Dialect::OpenaiChat | Dialect::OpenaiResponses => vec![
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
