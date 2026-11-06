//! Instance administration of runners (`/v1/admin/runners`, docs/05 §Instance administration).
mod dto;
mod routes;
pub mod service;

pub use routes::router;
