//! OIDC sign-in: PKCE, nonce, encrypted state bound to a cookie, identity linking, JIT (BE-015).
mod client;
mod roles;
mod routes;
pub mod service;

pub use client::OidcClients;
pub use routes::router;
