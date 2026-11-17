//! Evals (phase 5, P4-EVAL, docs/10 §Evals): P4-EVAL-1 is the questions, the agent's eval
//! settings and the configuration fingerprint. Runs, scoring and the judge follow in
//! P4-EVAL-2 and P4-EVAL-3.
pub mod dto;
pub mod fingerprint;
mod routes;
pub mod service;

pub use routes::router;
