//! Plugin package ingestion, trusted keys and package listing (BE-040).
pub mod claims;
pub mod dto;
pub mod ingest;
pub mod internal;
pub mod keys;
pub mod lifecycle;
pub mod registry;
mod routes;
pub mod scan;
pub mod service;
pub mod upgrade;

pub use routes::router;
