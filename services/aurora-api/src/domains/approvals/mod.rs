//! Approvals API: list, long-poll, SSE stream, decisions, batches (BE-030).
//! Transcript events (`approval.pending`/`approval.decided`) and channel cards
//! are M4/M6; `service::after_decision` is the seam.
pub mod bus;
pub mod dto;
mod routes;
pub mod service;

pub use routes::router;
