//! The Files page's backend (phase 5, P5-FILES-1, D-316, SEC-125, SEC-126; C-898..C-903,
//! C-906; docs/16 §W21). The API is an SFTP client of the sandbox image's `sftp-server`, started
//! by the supervisor as an `exec` in mode `sftp` over the runner's :4108 listener with a
//! single-use `auf_` grant. No gate of its own: the routes exist where the web terminal does.
//! File contents are never logged, stored or audited (paths and sizes only).
pub mod dto;
pub mod paths;
pub mod service;
pub mod sftp;

mod routes;

pub use routes::{content_disposition, router};
