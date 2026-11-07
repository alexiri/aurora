//! OAuth clients at workspace, organization and instance scope (BE-042, 08 §OAuth clients).
pub mod dto;
mod routes;
pub mod service;

pub use routes::router;
