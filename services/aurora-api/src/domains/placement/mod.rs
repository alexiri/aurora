//! New-user placement (P4-ONB-1, D-140, SEC-79): `GET/PUT /v1/admin/placement`, the template
//! preview, and the sign-in step (`service::apply_signin`).
pub mod dto;
mod routes;
pub mod service;
pub mod template;

pub use routes::router;
