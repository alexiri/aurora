//! Freeze / unfreeze of agents, workspaces and organizations (phase 2, RFE-006, D-50;
//! docs/04 §Freeze). `dto` (wire types), `service` (authority, effects, audit) and `routes`.
pub mod dto;
mod routes;
pub mod service;

pub use routes::router;
