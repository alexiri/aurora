//! Organizations, members, workspaces, access bindings, slug resolution (BE-012).
pub mod dto;
pub mod pending_access;
mod routes;
pub mod self_grant;
pub mod service;

use crate::app::AppState;
use utoipa_axum::router::OpenApiRouter;

pub fn router() -> OpenApiRouter<AppState> {
    routes::router().merge(pending_access::router())
}
