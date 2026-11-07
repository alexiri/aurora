//! Hosted-agent engine pieces (BE-052): model selection, context credentials, turn lifecycle
//! helpers, home-sync rendering, the due-work arms 4-8, supervisor report handlers, and the
//! sandbox/restart/models routes (contract M4).
pub mod dispatch;
pub mod dto;
pub mod home;
pub mod lifecycle;
pub mod model;
pub mod reports;
mod routes;

pub use routes::router;
