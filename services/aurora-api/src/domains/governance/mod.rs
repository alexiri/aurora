//! Organization governance API (BE-034): document read/write against the instance ceiling,
//! effective view, instance view, and re-evaluation of stored LLM key origins on change.
pub mod dto;
mod routes;
pub mod service;

pub use routes::router;
