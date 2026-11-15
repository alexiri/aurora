//! Relay plane (phase 4, P4-PRV-2, D-146, D-147, SEC-81/85; docs/04 §Preview access,
//! `16 §W17`): the `aul_`-authenticated surface the relay calls, the runner-plane
//! preview-stream grant endpoints, and the dashboard-side `GET /v1/previews/authorize`.
pub mod auth;
pub mod dto;
pub mod quota;
mod routes;
pub mod service;

pub use routes::router;
