//! Shared helpers for W5-W7 wire types: validation errors, redacted secrets,
//! and size measurement.

use std::fmt;
use std::io;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Sender-side bound violation. Senders MUST validate before sending because
/// receivers parse batches all-or-nothing.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ValidationError {
    #[error("{field}: {actual} exceeds maximum {max}")]
    TooLarge {
        field: &'static str,
        max: usize,
        actual: usize,
    },
    #[error("{field}: invalid ({reason})")]
    Invalid {
        field: &'static str,
        reason: &'static str,
    },
}

/// Messages that can check their own sender-side bounds.
pub trait Validate {
    fn validate(&self) -> Result<(), ValidationError>;
}

/// Fails when `actual > max`.
pub fn check_max(field: &'static str, actual: usize, max: usize) -> Result<(), ValidationError> {
    if actual > max {
        Err(ValidationError::TooLarge { field, max, actual })
    } else {
        Ok(())
    }
}

/// Fails when `s` has more than `max` characters.
pub fn check_chars(field: &'static str, s: &str, max: usize) -> Result<(), ValidationError> {
    // Byte length is an upper bound on char count; only count when needed.
    if s.len() > max {
        check_max(field, s.chars().count(), max)
    } else {
        Ok(())
    }
}

/// Fails when `s` has more than `max` UTF-8 bytes.
pub fn check_bytes(field: &'static str, s: &str, max: usize) -> Result<(), ValidationError> {
    check_max(field, s.len(), max)
}

struct Counter(usize);

impl io::Write for Counter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 += buf.len();
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Serialized JSON size in bytes (0 if the value cannot be serialized).
pub fn json_len<T: Serialize>(value: &T) -> usize {
    let mut c = Counter(0);
    match serde_json::to_writer(&mut c, value) {
        Ok(()) => c.0,
        Err(_) => 0,
    }
}

/// A secret-bearing string: serializes transparently, never printed by `Debug`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// The secret value. Callers must not log it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString([redacted])")
    }
}

/// An empty JSON object body (`{}`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Empty {}
