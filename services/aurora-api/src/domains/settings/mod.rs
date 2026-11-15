//! Instance settings (phase 5, SET-1, D-236..D-238, SEC-89, SEC-90): instance-wide configuration
//! as application state — registry-driven, locked where the deployment configuration sets a key,
//! validated on the merged view, live on every replica, audited. `GET/PUT /v1/admin/settings/…`.
pub mod cli;
pub mod dto;
pub mod handle;
mod routes;
pub mod rules;
pub mod sections;
pub mod service;

pub use routes::router;
