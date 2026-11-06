//! Organization app availability: posture and allow-list (BE-042, 07 §App availability).
pub mod dto;
mod routes;
pub mod service;

pub use routes::router;
