//! Email/password authentication flows and the mailer (BE-011).
pub mod dto;
pub mod mail;
pub mod password;
mod routes;
pub mod service;

pub use routes::router;
