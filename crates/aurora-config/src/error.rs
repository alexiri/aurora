//! Configuration errors. Every error names the offending key.

use std::fmt;

/// One problem, attached to a dotted key path (or an env var name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub key: String,
    pub message: String,
}

impl ConfigError {
    pub fn new(key: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.key, self.message)
    }
}

/// All problems found, in discovery order. Never empty.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct ConfigErrors(pub Vec<ConfigError>);

impl ConfigErrors {
    pub fn errors(&self) -> &[ConfigError] {
        &self.0
    }

    /// True when some error is attached to exactly `key`.
    pub fn has_key(&self, key: &str) -> bool {
        self.0.iter().any(|e| e.key == key)
    }
}

impl fmt::Display for ConfigErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid configuration ({} error(s))", self.0.len())?;
        for e in &self.0 {
            write!(f, "\n  - {e}")?;
        }
        Ok(())
    }
}

pub type Result<T> = std::result::Result<T, ConfigErrors>;
