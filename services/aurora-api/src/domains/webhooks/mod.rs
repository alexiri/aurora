//! Webhooks (BE-063): capability-URL receivers that create a turn in the webhook's own
//! conversation. Management API, public receiver, verification and failure bookkeeping.
pub mod dto;
pub mod receive;
mod routes;
pub mod service;

pub use routes::router;
pub use service::on_turn_finished;
