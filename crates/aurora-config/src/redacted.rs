//! Redacting wrapper for secret-valued configuration fields.

use serde::Deserialize;
use std::fmt;

/// Holds a value whose `Debug` output is always `[redacted]`.
#[derive(Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(transparent)]
pub struct Redacted<T>(T);

impl<T> Redacted<T> {
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// Access the secret value. Callers must not log it.
    pub fn expose(&self) -> &T {
        &self.0
    }
}

impl<T> fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}
