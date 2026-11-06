//! aurora-runner: sandbox runner core (`docs/11-runner-sandboxes.md`, `docs/16 §W5`).
//!
//! Outbound-only towards the API; never touches the database. Substrate access is
//! confined to `backends/<id>/` behind the [`backend::Backend`] trait.
#![forbid(unsafe_code)]

pub mod api;
pub mod backend;
pub mod backends;
pub mod executor;
pub mod hub;
pub mod reconcile;
pub mod reporter;
pub mod runner;

pub use runner::{Runner, RunnerSettings};
