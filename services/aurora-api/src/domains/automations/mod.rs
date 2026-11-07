//! RFE-021 (phase 2, D-58): reassigning schedules/webhooks, workspace anchors and the
//! offboarding view (docs/04 §Fire-time authority).
pub mod dto;
pub mod routes;
pub mod service;

pub use routes::router;
