//! Platform-level action approvals (BE-065, docs/07 §Action approvals): privileged agent
//! actions that are not HTTP requests. A handler registry keyed by `action_key` executes the
//! frozen payload after a person approves. Separate from the gateway `approvals` domain.
pub mod dto;
pub mod handlers;
mod routes;
pub mod service;

pub use routes::router;
