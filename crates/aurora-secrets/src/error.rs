/// Secret-store errors. No variant ever carries plaintext (SEC-01).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SecretError {
    #[error("secret reference does not match the purpose, owner or org it was sealed for")]
    SecretRefMismatch,
    #[error("secret store unavailable")]
    StoreUnavailable,
    #[error("secret store not configured")]
    StoreNotConfigured,
    #[error("secret store does not support references")]
    ReferencesNotSupported,
    #[error("secret store does not support managed values")]
    ManagedNotSupported,
    #[error("reference path not permitted for this tenant")]
    ReferenceNotPermitted,
    #[error("malformed secret reference or value: {0}")]
    Malformed(&'static str),
    #[error("secret not found")]
    NotFound,
    #[error("invalid secrets configuration: {0}")]
    InvalidConfig(String),
    #[error("secret store internal error")]
    Internal,
}

pub type Result<T, E = SecretError> = std::result::Result<T, E>;
