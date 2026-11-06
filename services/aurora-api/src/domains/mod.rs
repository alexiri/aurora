//! Domain modules: each exposes `pub fn router() -> OpenApiRouter<AppState>` and keeps
//! handlers (`routes.rs`) thin over `service.rs`; SQL lives in `aurora_db::repo`.

pub mod agents;
pub mod auth;
pub mod instance;
pub mod tenancy;
