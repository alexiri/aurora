//! Emergency read access (phase 5, FIX-236, D-478, C-1164, C-1165, SEC-154): break-glass read
//! access for organization owners. An org owner opens it, in a session, for one workspace, with
//! a reason, for 1–24 hours; the workspace's people and the org's other owners are told at once;
//! every conversation read is recorded before content is returned.

pub mod dto;
pub mod notify;
mod routes;
pub mod service;

pub use routes::router;
