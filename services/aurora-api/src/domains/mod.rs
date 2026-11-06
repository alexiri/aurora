//! Domain modules: each exposes `pub fn router() -> OpenApiRouter<AppState>` and keeps
//! handlers (`routes.rs`) thin over `service.rs`; SQL lives in `aurora_db::repo`.

pub mod activity;
pub mod admin;
pub mod agents;
pub mod approvals;
pub mod audit;
pub mod auth;
pub mod cli_login;
pub mod governance;
pub mod grants;
pub mod instance;
pub mod invitations;
pub mod keys;
pub mod me;
pub mod oidc;
pub mod plugins;
pub mod policy;
pub mod secrets;
pub mod tenancy;
