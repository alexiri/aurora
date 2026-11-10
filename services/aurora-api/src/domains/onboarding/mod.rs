//! Onboarding (P4-ONB-2, D-141): `GET/PATCH /v1/me/onboarding` and `GET /v1/admin/setup`.
pub mod dto;
mod routes;
pub mod service;

pub use routes::router;
