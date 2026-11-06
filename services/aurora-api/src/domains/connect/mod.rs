//! Connect flows (BE-043): fields + probe, oauth2 (PKCE, encrypted state + cookie), hook
//! methods (W9 client), callback, reconnect, scope diff, blocklist rules (09 §11).
pub mod dto;
pub mod env;
pub(crate) mod fields;
mod flow;
mod hooks;
mod page;
mod probe;
mod routes;
pub mod service;

pub use env::Env;
pub use routes::router;
