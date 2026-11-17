//! Evals (phase 5, P4-EVAL, docs/10 §Evals): P4-EVAL-1 is the questions, the agent's eval
//! settings and the configuration fingerprint; P4-EVAL-2 is the runs (real turns, one `eval`
//! conversation per question), scoring, side-effect refusal and eval schedules. The judge
//! follows in P4-EVAL-3.
pub mod dto;
pub mod engine;
pub mod fingerprint;
pub mod judge;
mod routes;
mod run_routes;
pub mod runs;
pub mod schedules;
pub mod scoring;
pub mod service;

pub fn router() -> utoipa_axum::router::OpenApiRouter<crate::app::AppState> {
    routes::router().merge(run_routes::router())
}
