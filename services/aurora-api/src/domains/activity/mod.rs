//! Workspace activity (request-log read API).
mod routes;

/// (P5-REFINE-9, C-1084) The timeline stream's `request` events carry this entry.
pub use routes::{RequestLogEntry, router};
