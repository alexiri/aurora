//! Terminator tests (`16 §W20`, SEC-92, SEC-95, D-356): an in-process `russh` client against the
//! real server, a stub of the API's terminator plane (axum) and a stub runner leg (WebSocket).
//! Covered: the authentication matrix (every refusal the same `USERAUTH_FAILURE` listing
//! `publickey` after the same delay, and **no API call** for a failing certificate), the `none`
//! probe uncounted and the sixth-attempt cap, the pre-auth caps and the auth timeout, the channel
//! and request allow-list (refused ones open nothing downstream), the leg round trip with exit
//! status, notices before disconnect (idle and revoked), the heartbeat fail-closed rule.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::result_large_err
)]

mod common;
mod matrix;
mod session;
