//! SSH access to hosted agents' sandboxes (phase 5, P4-SSH-1, D-239, SEC-92..SEC-96;
//! C-790..C-792): people register `ssh-ed25519` public keys and mint short-lived OpenSSH user
//! certificates signed by the instance's SSH CA; P4-SSH-2's terminator accepts them. Every
//! route answers 404 `ssh_disabled` while `ssh.enabled` is off.
mod dto;
pub mod keys;
pub mod plane;
mod routes;
pub mod service;
pub mod sessions;
pub mod signer;

pub use routes::router;
