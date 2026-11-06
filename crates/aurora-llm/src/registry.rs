use crate::http_util::host_of;
use crate::{
    AnthropicProvider, Dialect, Endpoint, KeyOriginRule, LitellmProvider, LlmError, LlmProvider,
    Result,
};
use aurora_config::{LlmEndpoint, LlmSection, RawConfig};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Core provider registry plus the resolved endpoints.
pub struct ProviderRegistry {
    providers: BTreeMap<&'static str, Arc<dyn LlmProvider>>,
    endpoints: Vec<Endpoint>,
}

fn builtin(id: &str) -> Option<Arc<dyn LlmProvider>> {
    match id {
        "litellm" => Some(Arc::new(LitellmProvider)),
        "anthropic" => Some(Arc::new(AnthropicProvider::default())),
        _ => None,
    }
}

impl ProviderRegistry {
    pub fn from_config(cfg: &LlmSection) -> Result<Self> {
        Self::build(cfg, builtin)
    }

    /// Like [`Self::from_config`] with a replaced Anthropic base URL (tests only).
    pub fn from_config_with_anthropic_base(cfg: &LlmSection, base: &str) -> Result<Self> {
        let base = base.to_string();
        Self::build(cfg, move |id| match id {
            "anthropic" => {
                Some(Arc::new(AnthropicProvider::with_base_url(base.clone()))
                    as Arc<dyn LlmProvider>)
            }
            other => builtin(other),
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
            });
        }
        Ok(Self {
            providers,
            endpoints,
        })
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

    /// `host` may carry `:port`; a pattern without a port matches any port.
    pub fn is_endpoint_host(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        let bare = host.split_once(':').map_or(host.as_str(), |(h, _)| h);
        self.endpoint_hosts()
            .iter()
            .any(|p| *p == host || (!p.contains(':') && p == bare))
    }
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
    Ok(Endpoint {
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
    })
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
