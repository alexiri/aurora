//! Secrets and LLM keys (BE-031): generic + LLM secrets through `SecretVault`, governance
//! admission, key validation and key-origin verification.
pub mod dto;
pub mod gov;
mod llm;
mod routes;
pub mod service;

pub use routes::router;
