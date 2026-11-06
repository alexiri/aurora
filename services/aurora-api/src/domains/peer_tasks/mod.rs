//! Peer tasks (BE-065, docs/07 §Agent links and peer tasks): one agent asks another for work on
//! behalf of a person; the receiver reports back exactly once.
pub mod dto;
mod routes;
pub mod service;

pub use routes::router;
