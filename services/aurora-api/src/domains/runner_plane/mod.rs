//! Runner plane `/v1/runner/*` (W5, docs/16): register, work long-poll (the due-work engine,
//! docs/05), reports, channel claim/hold/release, reconcile calls, retire. (P4-PRV-2) adds
//! the preview-stream grant endpoints (`preview_streams`).
//!
//! Machine plane: authenticated with the runner anchor token `aur_…` (+ `X-Aurora-Replica`),
//! deliberately **not** part of the dashboard OpenAPI document (routes are added with
//! `OpenApiRouter::route`, which carries no schema).
pub(crate) mod auth;
pub(crate) mod boot;
pub(crate) mod preview_streams;
mod routes;
pub mod service;
pub(crate) mod work;

pub use auth::RunnerCtx;
pub use routes::router;
