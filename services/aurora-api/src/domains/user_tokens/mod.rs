//! A person's chat-platform token and `send_as_me` (phase 5, P4-UTOK, D-281..D-284, C-853..C-858,
//! SEC-112, SEC-113; docs/13 §5.11): the owner adds a token of their own account (sealed, verified
//! against their link), grants single agents, and approves each post an agent proposes; a send is
//! a `message.post` the connector makes with the token for that one operation.
//!
//! * [`custody`]: the only place that seals, opens or discards the token and its per-op copies;
//! * [`claim`]: the connector claim's branch for an op that acts with a token;
//! * [`end`]: every end of the person's standing, in the trigger's own transaction;
//! * [`send`]: the `send_as_me` tool's eligibility, its approval and the approved send;
//! * [`events`]: the audit rows of a send.
pub mod claim;
pub mod custody;
mod dto;
pub mod end;
pub mod events;
mod routes;
pub mod send;
pub mod service;

pub use routes::router;
