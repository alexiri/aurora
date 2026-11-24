//! Webhooks (BE-063): capability-URL receivers that create a turn in the webhook's own
//! conversation. Management API, public receiver, verification and failure bookkeeping.
pub mod dto;
pub mod receive;
mod routes;
pub mod service;

pub use routes::router;
pub use service::on_turn_finished;
// (P6-TPL-3) The from-template create seals before its transaction and inserts inside it.
pub use service::{Create, PreparedWebhook, insert_in_tx, prepare};
pub(crate) use service::{discard_for, stored_ref};
