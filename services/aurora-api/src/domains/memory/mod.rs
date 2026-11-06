//! Agent memory: CRUD with revisions, redaction, query-time search, turn-start recall and the
//! harvest write (docs/10 §Memory). Truth lives here; sandboxes hold a projection.
pub mod dto;
mod routes;
pub mod service;

pub use routes::router;
pub use service::{Author, Source, Write, delete, memory_write, recall_snippets, save, search};
