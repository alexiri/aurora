use thiserror::Error;

pub type Result<T, E = LlmError> = std::result::Result<T, E>;

/// Errors never carry the API key or request/response bodies.
#[derive(Debug, Error)]
pub enum LlmError {
    #[error("llm endpoint unreachable: {0}")]
    Unreachable(String),
    #[error("llm key rejected by provider")]
    InvalidKey,
    #[error("key origin could not be verified: {0}")]
    Unverifiable(String),
    #[error("llm endpoint returned HTTP {0}")]
    Status(u16),
    #[error("unexpected response from llm endpoint: {0}")]
    BadResponse(String),
    #[error("provider does not support origin method {0}")]
    UnsupportedMethod(&'static str),
    #[error("llm configuration error: {0}")]
    Config(String),
}
