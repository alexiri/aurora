//! Session helpers, sign-out and the mailer (BE-011). Phase 1 (RFE-004): sign-in itself is OIDC
//! only (`domains::oidc`); the password, verification and reset routes are gone.
pub mod dto;
pub mod mail;
mod routes;
pub mod service;

pub use routes::router;
