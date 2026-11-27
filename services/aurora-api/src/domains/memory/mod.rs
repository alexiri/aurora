//! Agent memory: CRUD with revisions, redaction, query-time search, turn-start recall and the
//! harvest write (docs/10 §Memory). Truth lives here; sandboxes hold a projection.
mod actor;
pub mod dto;
pub mod events;
mod routes;
pub mod service;

pub use routes::router;
pub use service::{
    Author, Recalled, Source, Write, delete, mark_used, memory_write, recall_snippets, save, search,
};
