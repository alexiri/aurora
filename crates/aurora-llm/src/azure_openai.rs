use crate::http_util::{check, host_of, json, send};
use crate::mint::{self, MintClass, MintCtx};
use crate::{
    AuthMode, Dialect, Endpoint, EndpointKind, InjectionHeader, KeyOrigin, LlmError, LlmInjection,
    LlmProvider, ModelInfo, OriginMethod, Overrides, Result,
};
use aurora_core::Secret;
use http::{HeaderMap, Method};
use std::time::SystemTime;

const PLACEHOLDER: &str = "aurora-placeholder";

/// Azure OpenAI (`08 §Phase 4 providers`, C-800, C-804, C-806, C-809, C-810; D-261(c)): the operator-configured resource
/// host (`https://<resource>.openai.azure.com`), the OpenAI **v1** API under `/openai/v1/…` only (no `api-version`), the
/// `model` of a request is the *deployment name*. Two credentials: the resource key (`api-key`) and a Microsoft Entra
/// application whose client-credentials token the gateway mints (`Authorization: Bearer`, `mint::entra`).
///
/// Models are the entry's static deployment names (Azure's data plane lists base models, not deployments);
/// `GET /openai/v1/models` only proves that a credential works.
#[derive(Debug, Clone, Default)]
pub struct AzureOpenaiProvider {
    overrides: Overrides,
}

impl AzureOpenaiProvider {
    /// With `llm.dev_upstream_overrides` applied to the Entra authority (tests and development only).
    #[must_use]
    pub fn with_overrides(overrides: Overrides) -> Self {
        Self { overrides }
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

    /// One Entra mint: the evidence of the credential and of the tenant and application behind it.
    async fn mint(
        &self,
        ep: &Endpoint,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<mint::Minted> {
        let value = key.expose().as_bytes();
        mint::entra::parse_value(value)
            .map_err(|e| LlmError::Unverifiable(format!("key_invalid:{}", e.reason)))?;
        match mint::run(http, &mint::entra::EntraSpec, &self.ctx(ep, value)).await {
            Ok(m) => Ok(m),
            Err(e) if e.class == MintClass::Rejected => Err(LlmError::MintRejected),
            Err(_) => Err(LlmError::Unreachable("token endpoint unavailable".into())),
        }
    }

    /// `GET <base>/openai/v1/models` with the credential: the cheapest proof that the resource accepts it.
    async fn prove(
        &self,
        ep: &Endpoint,
        mode: AuthMode,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<()> {
        let url = format!("{}/openai/v1/models", ep.base());
        let rb = match mode {
            AuthMode::AzureEntra => {
                let m = self.mint(ep, key, http).await?;
                http.get(url).bearer_auth(m.material.expose())
            }
            _ => http.get(url).header("api-key", key.expose()),
        };
        let resp = check(send(rb).await?)?;
        let v = json(resp).await?;
        v.get("data")
            .and_then(|d| d.as_array())
            .map(|_| ())
            .ok_or_else(|| LlmError::BadResponse("missing `data` array".into()))
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

#[async_trait::async_trait]
impl LlmProvider for AzureOpenaiProvider {
    fn id(&self) -> &'static str {
        "azure_openai"
    }
    fn display_name(&self) -> &'static str {
        "Azure OpenAI"
    }
    fn endpoint_kind(&self) -> EndpointKind {
        EndpointKind::Configured
    }
    fn auth_modes(&self) -> &'static [AuthMode] {
        &[AuthMode::ApiKey, AuthMode::AzureEntra]
    }
    fn dialects(&self, ep: &Endpoint) -> Vec<Dialect> {
        if ep.dialects.is_empty() {
            vec![Dialect::OpenaiChat, Dialect::OpenaiResponses]
        } else {
            ep.dialects.clone()
        }
    }
    fn hosts(&self, ep: &Endpoint) -> Vec<String> {
        host_of(&ep.base_url).into_iter().collect()
    }
    fn injection(&self, mode: &AuthMode) -> LlmInjection {
        // Either way the agent's own credential headers go first (C-806).
        let header = match mode {
            AuthMode::AzureEntra => InjectionHeader::EntraToken,
            _ => InjectionHeader::Header("api-key"),
        };
        LlmInjection::new(header, &["authorization", "x-api-key", "api-key"])
    }
    fn inference_path_permitted(&self, ep: &Endpoint, method: &Method, path: &str) -> bool {
        crate::paths::azure_path_permitted(method, path, &ep.allowed_paths)
    }
    async fn list_models(
        &self,
        ep: &Endpoint,
        _key: &Secret<String>,
        _http: &reqwest::Client,
    ) -> Result<Vec<ModelInfo>> {
        Ok(static_models(ep))
    }
    async fn list_models_strict(
        &self,
        ep: &Endpoint,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<Vec<ModelInfo>> {
        self.list_models_strict_as(ep, AuthMode::ApiKey, key, http)
            .await
    }
    async fn list_models_strict_as(
        &self,
        ep: &Endpoint,
        mode: AuthMode,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<Vec<ModelInfo>> {
        match self.prove(ep, mode, key, http).await {
            // A value that is not an Entra application is a bad key, not an unverifiable origin.
            Err(LlmError::Unverifiable(_)) => Err(LlmError::InvalidKey),
            Err(e) => Err(e),
            Ok(()) => Ok(static_models(ep)),
        }
    }
    fn default_model(&self, ep: &Endpoint) -> Option<String> {
        ep.default_model.clone()
    }
    fn client_env(
        &self,
        ep: &Endpoint,
        _dialect: Dialect,
        _mode: &AuthMode,
    ) -> Vec<(String, String)> {
        vec![
            ("OPENAI_BASE_URL".into(), format!("{}/openai/v1", ep.base())),
            ("OPENAI_API_KEY".into(), PLACEHOLDER.into()),
        ]
    }

    fn origin_methods(&self) -> &'static [OriginMethod] {
        &[OriginMethod::Endpoint, OriginMethod::ProviderAccount]
    }
    async fn verify_key_origin(
        &self,
        ep: &Endpoint,
        method: OriginMethod,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<KeyOrigin> {
        self.verify_key_origin_as(ep, AuthMode::ApiKey, method, key, http)
            .await
    }
    async fn verify_key_origin_as(
        &self,
        ep: &Endpoint,
        mode: AuthMode,
        method: OriginMethod,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<KeyOrigin> {
        match (method, mode) {
            (OriginMethod::Endpoint, _) => {
                self.prove(ep, mode, key, http).await?;
                Ok(KeyOrigin::default())
            }
            // The mint proves the tenant and the application (C-804).
            (OriginMethod::ProviderAccount, AuthMode::AzureEntra) => self
                .mint(ep, key, http)
                .await?
                .origin
                .ok_or_else(|| LlmError::Unverifiable("no_tenant".into())),
            // A resource key says nothing about whose it is (C-804).
            (OriginMethod::ProviderAccount, _) => {
                Err(LlmError::Unverifiable("api_key_reports_no_account".into()))
            }
            (other, _) => Err(LlmError::UnsupportedMethod(other.as_str())),
        }
    }
    fn origin_from_response(&self, _ep: &Endpoint, _headers: &HeaderMap) -> Option<KeyOrigin> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep() -> Endpoint {
        Endpoint {
            id: "az".into(),
            provider: "azure_openai".into(),
            display_name: "Azure".into(),
            base_url: "https://acme.openai.azure.com/".into(),
            dialects: vec![],
            default_model: Some("gpt-4o".into()),
            models: vec!["gpt-4o".into()],
            allowed_models: vec![],
            key_origin: None,
            allowed_paths: vec![],
            organization: None,
            project: None,
            auth_header: None,
            models_path: None,
            region: None,
            project_id: None,
            location: None,
        }
    }

    #[test]
    fn injection_follows_the_mode() {
        let p = AzureOpenaiProvider::default();
        let k = p.injection(&AuthMode::ApiKey);
        assert_eq!(k.header, InjectionHeader::Header("api-key"));
        let e = p.injection(&AuthMode::AzureEntra);
        assert_eq!(e.header, InjectionHeader::EntraToken);
        for i in [k, e] {
            assert!(i.strip_request_headers.contains(&"authorization"));
            assert!(i.strip_request_headers.contains(&"api-key"));
        }
    }

    #[test]
    fn env_and_hosts_follow_the_base() {
        let p = AzureOpenaiProvider::default();
        let env = p.client_env(&ep(), Dialect::OpenaiResponses, &AuthMode::ApiKey);
        assert!(env.contains(&(
            "OPENAI_BASE_URL".into(),
            "https://acme.openai.azure.com/openai/v1".into()
        )));
        assert_eq!(p.hosts(&ep()), ["acme.openai.azure.com"]);
    }
}
