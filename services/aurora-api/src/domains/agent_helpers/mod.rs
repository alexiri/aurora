//! Agent helper files (P6-TPL-6, C-1143): a template's helper files stored with an agent and
//! projected read-only at `.aurora/helpers/<path>` by the supervisor (12 §Home projection).
//! The writer (`replace_all`, P6-TPL-3's caller) and the remove path live in `service`; the
//! list/read/remove routes of C-1143 in `routes`. No agent tool exists for any of this: the
//! agent's computer only ever reads the projection.
pub mod dto;
mod routes;
pub mod service;

pub use routes::router;
