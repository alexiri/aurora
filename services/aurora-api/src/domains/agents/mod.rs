//! Agents: read side (M1) plus BYO lifecycle, token and bootstrap (M2).
pub mod avatar;
pub mod byo;
pub mod dto;
mod routes;
// (P6-TPL-3) The from-template create runs the agent's insert inside its own transaction.
pub(crate) mod service;

pub use routes::router;
