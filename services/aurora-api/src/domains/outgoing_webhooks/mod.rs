//! Outgoing webhooks (phase 5, P5-WEBHOOK-2, D-320, C-909, C-912, SEC-127): the configuration of a
//! send-mode connection (read, edit), the preview, the presets and the send test. Connect itself is
//! the connect flow's (`connect::service::webhook`).
pub mod dto;
mod routes;
mod send;
pub mod service;

pub use routes::router;
