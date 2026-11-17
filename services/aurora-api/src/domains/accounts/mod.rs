//! Accounts (P5-UX-4, D-345, C-917): one list of what agents can use, over connections, secrets
//! and grants. The routes of the separate lists stay for the API.
pub mod dto;
pub mod gateway;
pub mod health;
mod routes;
pub mod service;
pub mod test;

pub use routes::router;
