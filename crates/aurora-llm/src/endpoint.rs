use crate::{Dialect, KeyOriginRule};

/// A configured (or implicit) provider endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub id: String,
    pub provider: String,
    pub display_name: String,
    pub base_url: String,
    /// Empty = the provider default.
    pub dialects: Vec<Dialect>,
    pub default_model: Option<String>,
    /// Static fallback model list.
    pub models: Vec<String>,
    /// Instance-wide model allow-list; empty = any.
    pub allowed_models: Vec<String>,
    pub key_origin: Option<KeyOriginRule>,
    /// (D-177, FIX-105) Extra paths the sanctioned key may be injected on, beyond the inference
    /// set (`litellm`, `openai_compatible`); absolute paths, a trailing `/*` allows a subtree.
    pub allowed_paths: Vec<String>,
    /// `openai`: `OpenAI-Organization` / `OpenAI-Project` the gateway sets (C-800).
    pub organization: Option<String>,
    pub project: Option<String>,
    /// `openai_compatible`: the header carrying the key (`authorization`, `api-key`, `x-api-key`) and the model
    /// listing path (absolute).
    pub auth_header: Option<String>,
    pub models_path: Option<String>,
    /// `bedrock` / `vertex` (P4-LLM-2/4): region, project id, location.
    pub region: Option<String>,
    pub project_id: Option<String>,
    pub location: Option<String>,
    /// (P5-MODELS-1, C-875) Operator-named models for the harness's roles (`large`, `medium`, `small`).
    pub model_roles: std::collections::BTreeMap<String, String>,
    /// (C-875) What the operator says about models the harness cannot recognise by id.
    pub model_info: std::collections::BTreeMap<String, aurora_config::ModelInfoEntry>,
}

impl Endpoint {
    pub(crate) fn base(&self) -> &str {
        self.base_url.trim_end_matches('/')
    }

    /// The path of `base_url` without a trailing slash (`""` when none): the prefix the inference paths are relative to.
    pub fn base_path(&self) -> String {
        self.base_url
            .parse::<http::Uri>()
            .map(|u| u.path().trim_end_matches('/').to_string())
            .unwrap_or_default()
    }
}
