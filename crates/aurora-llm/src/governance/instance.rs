//! Instance ceiling (08 §11.1) built from configuration and the provider registry.
use super::model::{AllowEntry, EgressPosture, LlmPosture};
use crate::{KeyOriginRule, LlmError, OriginMethod, ProviderRegistry, Result};
use aurora_config::RawConfig;

#[derive(Debug, Clone)]
pub struct InstanceEndpoint {
    pub id: String,
    pub allowed_models: Vec<String>,
    pub key_origin: Option<KeyOriginRule>,
    /// Host patterns (`host` or `host:port`) of the endpoint.
    pub hosts: Vec<String>,
    /// Origin methods the endpoint's provider supports.
    pub origin_methods: Vec<OriginMethod>,
}

#[derive(Debug, Clone)]
pub struct Instance {
    pub allowed_key_scopes: Vec<String>,
    pub llm_minimum_posture: LlmPosture,
    pub egress_minimum_posture: EgressPosture,
    pub require_key_origin: bool,
    pub endpoints: Vec<InstanceEndpoint>,
    /// Known LLM hosts (providers Aurora has no configured endpoint for).
    pub known_llm_hosts: Vec<String>,
    pub baseline_allowlist: Vec<AllowEntry>,
}

impl Instance {
    /// Build from the `llm` and `governance` sections of the raw configuration.
    pub fn from_config(cfg: &RawConfig, registry: &ProviderRegistry) -> Result<Self> {
        let g = &cfg.governance;
        let llm_minimum_posture = LlmPosture::parse(&g.llm.minimum_posture).ok_or_else(|| {
            LlmError::Config("governance.llm.minimum_posture: invalid value".into())
        })?;
        let egress_minimum_posture =
            EgressPosture::parse(&g.egress.minimum_posture).ok_or_else(|| {
                LlmError::Config("governance.egress.minimum_posture: invalid value".into())
            })?;
        let endpoints = registry
            .endpoints()
            .iter()
            .map(|ep| {
                let provider = registry.provider_for(ep);
                InstanceEndpoint {
                    id: ep.id.clone(),
                    allowed_models: ep.allowed_models.clone(),
                    key_origin: ep.key_origin.clone(),
                    hosts: provider.map(|p| p.hosts(ep)).unwrap_or_default(),
                    origin_methods: provider
                        .map(|p| p.origin_methods().to_vec())
                        .unwrap_or_default(),
                }
            })
            .collect();
        Ok(Self {
            allowed_key_scopes: g.llm.allowed_key_scopes.clone(),
            llm_minimum_posture,
            egress_minimum_posture,
            require_key_origin: g.llm.require_key_origin,
            endpoints,
            known_llm_hosts: cfg.known_llm_hosts(),
            baseline_allowlist: g
                .egress
                .baseline_allowlist
                .iter()
                .map(|e| AllowEntry {
                    host: e.host.clone(),
                    path: e.path.clone(),
                })
                .collect(),
        })
    }

    pub fn endpoint(&self, id: &str) -> Option<&InstanceEndpoint> {
        self.endpoints.iter().find(|e| e.id == id)
    }
}
