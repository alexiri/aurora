use crate::http_util::host_of;
use crate::{
    AnthropicProvider, AzureOpenaiProvider, Dialect, Endpoint, KeyOriginRule, LitellmProvider,
    LlmError, LlmProvider, OpenaiCompatibleProvider, OpenaiProvider, Overrides, Result,
    VertexProvider,
};
use aurora_config::{LlmEndpoint, LlmSection, RawConfig};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Core provider registry plus the resolved endpoints.
pub struct ProviderRegistry {
    providers: BTreeMap<&'static str, Arc<dyn LlmProvider>>,
    endpoints: Vec<Endpoint>,
    overrides: Overrides,
}

fn builtin(id: &str) -> Option<Arc<dyn LlmProvider>> {
    match id {
        "litellm" => Some(Arc::new(LitellmProvider)),
        "anthropic" => Some(Arc::new(AnthropicProvider::default())),
        "openai" => Some(Arc::new(OpenaiProvider::default())),
        "openai_compatible" => Some(Arc::new(OpenaiCompatibleProvider)),
        "vertex" => Some(Arc::new(VertexProvider::default())),
        "azure_openai" => Some(Arc::new(AzureOpenaiProvider::default())),
        _ => None,
    }
}

/// (C-812, D-260(e)) The provider table with `llm.dev_upstream_overrides` applied to the fixed bases: the target
/// replaces the origin (`https://api.openai.com` → `http://127.0.0.1:9`), so the endpoint's host, injection target
/// and requests all follow it. The config layer has already refused anything but loopback targets.
fn with_overrides(
    overrides: &BTreeMap<String, String>,
) -> impl Fn(&str) -> Option<Arc<dyn LlmProvider>> + '_ {
    move |id| {
        let target = |origin: &str| {
            overrides
                .get(origin)
                .map(|t| t.trim_end_matches('/').to_string())
        };
        match id {
            "anthropic" => Some(match target(crate::ANTHROPIC_BASE_URL) {
                Some(t) => Arc::new(AnthropicProvider::with_base_url(t)) as Arc<dyn LlmProvider>,
                None => Arc::new(AnthropicProvider::default()),
            }),
            "openai" => Some(match target(crate::OPENAI_BASE_URL) {
                Some(t) => Arc::new(OpenaiProvider::with_base_url(t)) as Arc<dyn LlmProvider>,
                None => Arc::new(OpenaiProvider::default()),
            }),
            "vertex" => Some(Arc::new(VertexProvider::with_overrides(Overrides::new(
                overrides,
            )))),
            "azure_openai" => Some(Arc::new(AzureOpenaiProvider::with_overrides(
                Overrides::new(overrides),
            ))),
            other => builtin(other),
        }
    }
}

impl ProviderRegistry {
    pub fn from_config(cfg: &LlmSection) -> Result<Self> {
        Self::build(cfg, with_overrides(&cfg.dev_upstream_overrides))
    }

    /// Like [`Self::from_config`] with a replaced Anthropic base URL (tests only).
    pub fn from_config_with_anthropic_base(cfg: &LlmSection, base: &str) -> Result<Self> {
        let base = base.to_string();
        let overrides = cfg.dev_upstream_overrides.clone();
        Self::build(cfg, move |id| match id {
            "anthropic" => {
                Some(Arc::new(AnthropicProvider::with_base_url(base.clone()))
                    as Arc<dyn LlmProvider>)
            }
            other => with_overrides(&overrides)(other),
        })
    }

    fn build(
        cfg: &LlmSection,
        make: impl Fn(&str) -> Option<Arc<dyn LlmProvider>>,
    ) -> Result<Self> {
        let mut providers = BTreeMap::new();
        for name in &cfg.enabled_providers {
            let p = make(name)
                .ok_or_else(|| LlmError::Config(format!("unknown enabled provider `{name}`")))?;
            providers.insert(p.id(), p);
        }
        let mut endpoints: Vec<Endpoint> = Vec::new();
        for e in &cfg.endpoints {
            let provider = providers.get(e.provider.as_str()).ok_or_else(|| {
                LlmError::Config(format!(
                    "endpoint `{}`: provider `{}` is unknown or not enabled",
                    e.id, e.provider
                ))
            })?;
            if endpoints.iter().any(|x| x.id == e.id) {
                return Err(LlmError::Config(format!(
                    "duplicate endpoint id `{}`",
                    e.id
                )));
            }
            endpoints.push(resolve(e, provider.as_ref())?);
        }
        if providers.contains_key("anthropic")
            && !endpoints.iter().any(|e| e.provider == "anthropic")
        {
            if endpoints.iter().any(|e| e.id == "anthropic") {
                return Err(LlmError::Config(
                    "endpoint id `anthropic` is reserved for provider anthropic".into(),
                ));
            }
            endpoints.push(Endpoint {
                id: "anthropic".into(),
                provider: "anthropic".into(),
                display_name: "Anthropic".into(),
                base_url: crate::ANTHROPIC_BASE_URL.into(),
                dialects: vec![Dialect::AnthropicMessages],
                default_model: None,
                models: vec![],
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
            });
        }
        Ok(Self {
            providers,
            endpoints,
            overrides: Overrides::new(&cfg.dev_upstream_overrides),
        })
    }

    /// (C-812) `llm.dev_upstream_overrides`, for the mint service and the API's validation.
    pub fn overrides(&self) -> &Overrides {
        &self.overrides
    }

    pub fn endpoint(&self, id: &str) -> Option<&Endpoint> {
        self.endpoints.iter().find(|e| e.id == id)
    }

    pub fn endpoints(&self) -> &[Endpoint] {
        &self.endpoints
    }

    pub fn provider(&self, id: &str) -> Option<&Arc<dyn LlmProvider>> {
        self.providers.get(id)
    }

    /// Provider of an endpoint.
    pub fn provider_for(&self, ep: &Endpoint) -> Option<&Arc<dyn LlmProvider>> {
        self.provider(&ep.provider)
    }

    /// Every host pattern of every endpoint (deduplicated, in endpoint order).
    pub fn endpoint_hosts(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for ep in &self.endpoints {
            if let Some(p) = self.provider_for(ep) {
                for h in p.hosts(ep) {
                    if !out.contains(&h) {
                        out.push(h);
                    }
                }
            }
        }
        out
    }

    /// (D-177, SEC-65) Whether the sanctioned key of endpoint `endpoint_id` may be injected into
    /// this request. Endpoints of providers without an inference-path set (`anthropic` until
    /// phase 5) and unknown endpoints are not restricted here.
    pub fn path_permitted(&self, endpoint_id: &str, method: &http::Method, path: &str) -> bool {
        let Some(ep) = self.endpoint(endpoint_id) else {
            return true;
        };
        // a provider that is not in the registry has no endpoint, so this is always `Some`
        self.provider_for(ep)
            .is_none_or(|p| p.inference_path_permitted(ep, method, path))
    }

    /// (FIX-104, D-180) Whether a key of `provider` (and `endpoint_id` when the secret names
    /// one) may be injected into a request that leaves over `https` (or plain HTTP) to
    /// `host:port`: only to the exact scheme, host and port an operator configured as the
    /// endpoint's base URL. Anything else fails closed.
    pub fn accepts_injection(
        &self,
        endpoint_id: Option<&str>,
        provider: Option<&str>,
        https: bool,
        host: &str,
        port: u16,
    ) -> bool {
        self.endpoints
            .iter()
            .filter(|e| match endpoint_id {
                Some(id) => e.id == id,
                None => provider.is_none_or(|p| e.provider == p),
            })
            .filter_map(|e| self.provider_for(e).map(|p| p.base_url(e)))
            .any(|b| origin_matches(&b, https, host, port))
    }

    /// `host` may carry `:port`; a pattern without a port matches any port.
    pub fn is_endpoint_host(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        let bare = host.split_once(':').map_or(host.as_str(), |(h, _)| h);
        self.endpoint_hosts()
            .iter()
            .any(|p| *p == host || (!p.contains(':') && p == bare))
    }
}

/// `base_url`'s scheme, host and port (the scheme's default when absent) equal the request's.
fn origin_matches(base_url: &str, https: bool, host: &str, port: u16) -> bool {
    let Ok(uri) = base_url.parse::<http::Uri>() else {
        return false;
    };
    let (Some(scheme), Some(h)) = (uri.scheme_str(), uri.host()) else {
        return false;
    };
    let base_https = match scheme {
        "https" => true,
        "http" => false,
        _ => return false,
    };
    let base_port = uri.port_u16().unwrap_or(if base_https { 443 } else { 80 });
    base_https == https
        && base_port == port
        && h.trim_matches(['[', ']'])
            .eq_ignore_ascii_case(host.trim_matches(['[', ']']))
}

fn resolve(e: &LlmEndpoint, provider: &dyn LlmProvider) -> Result<Endpoint> {
    let base_url = match provider.endpoint_kind() {
        crate::EndpointKind::Fixed(url) => {
            if e.base_url.is_some() {
                return Err(LlmError::Config(format!(
                    "endpoint `{}`: provider `{}` has a fixed base URL",
                    e.id, e.provider
                )));
            }
            url.to_string()
        }
        crate::EndpointKind::Derived => String::new(),
        crate::EndpointKind::Configured => {
            let b = e.base_url.clone().ok_or_else(|| {
                LlmError::Config(format!("endpoint `{}`: base_url is required", e.id))
            })?;
            if host_of(&b).is_none() {
                return Err(LlmError::Config(format!(
                    "endpoint `{}`: base_url is invalid",
                    e.id
                )));
            }
            b
        }
    };
    let key_origin = e
        .key_origin
        .as_ref()
        .map(KeyOriginRule::from_config)
        .transpose()
        .map_err(|err| LlmError::Config(format!("endpoint `{}`: {err}", e.id)))?;
    if let Some(r) = &key_origin
        && !provider.origin_methods().contains(&r.method)
    {
        return Err(LlmError::Config(format!(
            "endpoint `{}`: provider `{}` does not support key_origin method `{}`",
            e.id,
            e.provider,
            r.method.as_str()
        )));
    }
    let mut dialects = Vec::with_capacity(e.dialects.len());
    for d in &e.dialects {
        dialects.push(crate::Dialect::parse(d).ok_or_else(|| {
            LlmError::Config(format!("endpoint `{}`: unknown dialect `{d}`", e.id))
        })?);
    }
    let mut ep = Endpoint {
        id: e.id.clone(),
        provider: e.provider.clone(),
        display_name: e
            .display_name
            .clone()
            .unwrap_or_else(|| provider.display_name().to_string()),
        base_url,
        dialects,
        default_model: e.default_model.clone(),
        models: e.models.clone(),
        allowed_models: e.allowed_models.clone(),
        key_origin,
        allowed_paths: e.allowed_paths.clone(),
        organization: e.organization.clone(),
        project: e.project.clone(),
        auth_header: e.auth_header.clone(),
        models_path: e.models_path.clone(),
        region: e.region.clone(),
        project_id: e.project_id.clone(),
        location: e.location.clone(),
    };
    if provider.endpoint_kind() == crate::EndpointKind::Derived {
        ep.base_url = provider.base_url(&ep);
    }
    Ok(ep)
}

/// (C-802) The auth modes an endpoint's key-origin rule admits: an `azure_openai` endpoint with `provider_account` admits
/// `azure_entra` only (the token proves the tenant), a `bedrock` one `aws_assume_role` and `aws_sigv4` only (the
/// signed caller identity proves the account); every other combination admits the provider's modes.
pub fn endpoint_auth_modes(provider: &dyn LlmProvider, ep: &Endpoint) -> Vec<crate::AuthMode> {
    use crate::{AuthMode, OriginMethod};
    let provider_account = ep
        .key_origin
        .as_ref()
        .is_some_and(|r| r.method == OriginMethod::ProviderAccount);
    provider
        .auth_modes()
        .iter()
        .copied()
        .filter(|m| {
            !provider_account
                || match provider.id() {
                    "azure_openai" => *m == AuthMode::AzureEntra,
                    "bedrock" => matches!(m, AuthMode::AwsSigv4 | AuthMode::AwsAssumeRole),
                    _ => true,
                }
        })
        .collect()
}

/// (C-802) The mode the dashboard preselects: `aws_assume_role` for Bedrock, else the first.
pub fn recommended_auth_mode(provider: &dyn LlmProvider) -> Option<crate::AuthMode> {
    if provider
        .auth_modes()
        .contains(&crate::AuthMode::AwsAssumeRole)
    {
        return Some(crate::AuthMode::AwsAssumeRole);
    }
    provider.auth_modes().first().copied()
}

/// Governed LLM hosts: endpoint hosts ∪ known LLM hosts (deduplicated, case-insensitive).
pub fn governed_hosts(registry: &ProviderRegistry, cfg: &RawConfig) -> Vec<String> {
    let mut out = registry.endpoint_hosts();
    for h in cfg.known_llm_hosts() {
        if !out.iter().any(|o| o.eq_ignore_ascii_case(&h)) {
            out.push(h);
        }
    }
    out
}
