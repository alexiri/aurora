//! Instance administration: users, bootstrap-admin (BE-016).
pub mod attention;
pub mod dto;
mod routes;
pub mod secret_stores;
pub mod service;

pub use routes::router;
