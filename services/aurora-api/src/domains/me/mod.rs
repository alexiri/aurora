//! `/v1/me`: profile, default org, account deletion (BE-016).
pub mod dto;
mod routes;
pub mod service;

pub use routes::router;
