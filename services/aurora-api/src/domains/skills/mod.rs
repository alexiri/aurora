//! Skills: agent-, workspace- and organization-tier instruction bundles (docs/10 §Skills).
//! Truth lives here; sandboxes hold a read-only projection (hosted/home.rs). (Phase 4, P4-GSL)
//! A skill may come from a git skill library: pinned, read-only, updated only by a person
//! (`library`).
pub mod dto;
pub mod library;
mod routes;
pub mod service;

use crate::app::AppState;
use utoipa_axum::router::OpenApiRouter;

pub use service::{NewSkill, Patch, SkillView};

pub fn router() -> OpenApiRouter<AppState> {
    routes::router().merge(library::router())
}
