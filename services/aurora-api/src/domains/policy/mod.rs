//! Policy authoring (BE-033): drafts, publish, rollback, warnings and simulation.
//! Matching and evaluation live in `aurora_policy`; SQL in `aurora_db::repo::policy`.
pub mod dto;
mod routes;
pub mod service;
pub mod simulate;
pub mod validate;
pub mod warnings;

pub use routes::router;
