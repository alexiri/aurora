//! Agent links (BE-065, docs/07 §Agent links): per-side policy between two agents; the
//! `message_agent` action-approval handler lives here.
pub mod dto;
pub mod handler;
mod routes;
pub mod service;

pub use routes::router;
