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
}

impl Endpoint {
    pub(crate) fn base(&self) -> &str {
        self.base_url.trim_end_matches('/')
    }
}
