//! Plugin package ingestion, trusted keys and package listing (BE-040).
pub mod dto;
pub mod ingest;
pub mod keys;
pub mod registry;
mod routes;
pub mod scan;
pub mod service;

pub use routes::router;
