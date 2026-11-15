//! Organization governance API (BE-034): document read/write against the instance ceiling,
//! effective view, instance view, and re-evaluation of stored LLM key origins on change.
pub mod dto;
pub mod reeval;
mod routes;
pub mod service;
pub mod standing;

pub use routes::router;
