//! (P5-REFINE-8, D-417, D-407, C-1080, C-1082) One attention feed per level — workspace, organization,
//! Administration — with the same item shape, and the rails' counts read from the same sources.
//! `admin::attention` keeps C-1022's six kinds and calls [`service::admin_request_items`] for the
//! requests of the Administration level.
mod routes;
pub mod service;
pub mod text;

pub use routes::router;
