//! Conversation plane API (BE-051): conversations, turns, follow-ups, transcript persistence, SSE,
//! abort/send-now. Due-work dispatch and supervisor report handling are BE-052, which calls
//! [`service`] (`create_turn`, `append_event`, `finish_turn`, `promote_followups`, ...) and
//! [`bus::ConversationBus::publish_delta`].
pub mod bus;
pub mod dto;
pub mod lifecycle;
mod routes;
pub mod service;
mod zip;

pub use routes::router;
