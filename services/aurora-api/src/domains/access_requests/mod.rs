//! (P5-UX-17, D-348, C-984, SEC-132) An agent's request for access, as a record and a card.
//!
//! A request gives nothing. It is resolved by a real access change (`service::on_access_given`,
//! called by the one writer of connection grants, inside its transaction), dismissed by a person
//! who can own the workspace, or expired by the sweeper after `ACCESS_REQUEST_TTL`.
pub mod dto;
mod routes;
pub mod service;

pub use routes::router;
