//! Approvals API: list, long-poll, SSE stream, decisions, batches (BE-030).
//! Transcript events (`approval.pending`/`approval.decided`/`approval.expired`, P5-UX-16) are
//! written by `transcript`; the sentence, origin and Always allow of C-982 by `enrich` and
//! `service::decide`.
pub mod bus;
pub mod dto;
pub mod enrich;
pub mod notify;
pub mod resume;
mod routes;
pub mod service;
pub mod transcript;

pub use routes::router;
