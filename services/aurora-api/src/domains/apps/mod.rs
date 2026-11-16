//! App catalogue, detail, icon and setup guide, served from the active snapshot (BE-042, C-13).
mod assets;
pub mod dto;
pub mod guide_render;
mod routes;
pub mod service;
pub mod setup;

pub use routes::router;
