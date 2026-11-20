//! (phase 5, P5-REFINE-9, D-415; C-1083, C-1084, SEC-143) One live timeline per agent: what it
//! did, is doing and asked, in structured kinds and names the web turns into sentences.
pub mod computer;
pub mod dto;
pub mod feed;
mod routes;
pub mod stream;

pub use routes::router;
