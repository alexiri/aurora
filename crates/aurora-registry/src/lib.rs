//! aurora-registry — plugin manifest model, template language and linter
//! (docs/09-app-plugins.md §3–§6).
//!
//! Known gap (REG-002): the group-wildcard "true superset" check needs matcher
//! reasoning and is not implemented here.
#![forbid(unsafe_code)]

pub mod manifest;
pub mod template;
pub mod validate;

pub use manifest::Manifest;
pub use template::{Template, TemplateError};

/// One lint finding. `path` is JSON-pointer-like (`/apps/0/hosts/1/inject/0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub code: &'static str,
    pub path: String,
    pub message: String,
}

impl Violation {
    pub fn new(code: &'static str, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code,
            path: path.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at {}: {}", self.code, self.path, self.message)
    }
}
