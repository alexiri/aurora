//! Skills: agent- and organization-tier instruction bundles (docs/10 §Skills). Truth lives
//! here; sandboxes hold a read-only projection (hosted/home.rs).
pub mod dto;
mod routes;
pub mod service;

pub use routes::router;
pub use service::{NewSkill, Patch, SkillView};
