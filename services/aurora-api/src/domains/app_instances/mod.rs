//! Self-hosted app instances (docs/05 §App instances, docs/09 §3.4 Instance-bound hosts, D-74,
//! SEC-59): registration is the admin trust decision about where an app's credentials may go.
pub mod dto;
mod routes;
pub mod service;

pub use routes::router;
