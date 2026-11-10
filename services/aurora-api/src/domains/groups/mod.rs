//! Directory groups (P4-GRP-1, D-130, D-131, D-156, SEC-71): routes, the service and the sign-in
//! sync of IdP-linked groups. Access itself is decided only by `aurora_db::authz`.
pub mod dto;
mod routes;
pub mod service;
pub mod sync;

pub use routes::router;
