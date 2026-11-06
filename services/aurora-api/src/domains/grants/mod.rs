//! Grants (BE-033): secret grants compiled into derived rules, and the effective view.
//! Connection grants (M3) slot into `compile` and `GrantsResponse.connections`.
pub mod compile;
pub mod dto;
pub(crate) mod effective;
mod routes;
pub mod service;

pub use routes::router;
