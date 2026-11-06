//! Runner plane `/v1/runner/*` (W5, docs/16): register, work long-poll (the due-work engine,
//! docs/05), reports, channel claim/hold/release, reconcile calls, retire.
//!
//! Machine plane: authenticated with the runner anchor token `aur_…` (+ `X-Aurora-Replica`),
//! deliberately **not** part of the dashboard OpenAPI document (routes are added with
//! `OpenApiRouter::route`, which carries no schema).
mod auth;
pub(crate) mod boot;
mod routes;
pub mod service;
mod work;

pub use routes::router;
