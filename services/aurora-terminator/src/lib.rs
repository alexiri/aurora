//! aurora-terminator: the SSH front door (phase 5, P4-SSH-2, D-239, D-356; `16 §W20`).
//!
//! Certificate-only SSH access into hosted agents' sandboxes. The terminator holds no signing
//! key, no database, no Kubernetes or podman credential: it verifies the certificate offline,
//! asks the API (`/v1/terminator/*`) for every decision, and bridges each accepted channel to a
//! leg-1 WebSocket on the runner. Sessions are rows in the API with a lease; this process keeps
//! only an in-memory table, so a restart loses its connections and nothing else.
#![forbid(unsafe_code)]

pub mod cert;
pub mod leg;
pub mod limits;
pub mod plane;
pub mod server;
pub mod state;
