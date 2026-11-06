//! Agents: read side (M1) plus BYO lifecycle, token and bootstrap (M2).
pub mod dto;
mod routes;
mod service;

pub use routes::router;
