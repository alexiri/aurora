//! Attachments (BE-064; docs/10 §Attachments, docs/16 §W5, decision S-11): inbound uploads and
//! downloads on the control plane, the runner-plane byte routes (`runner.rs`), the [`BlobStore`]
//! and the hourly pending sweep.
pub mod blob;
pub mod dto;
mod routes;
pub mod runner;
pub mod service;

pub use routes::router;
pub use service::sweep_pending;
