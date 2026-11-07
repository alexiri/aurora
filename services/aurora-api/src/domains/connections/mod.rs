//! App connections: list/get/patch/delete (BE-042). Connect flows live elsewhere (BE-043).
pub mod dto;
mod routes;
pub mod service;

pub use routes::router;
