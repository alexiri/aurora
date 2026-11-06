//! The fake connector: a complete connector over a simulated in-memory platform
//! (`docs/13 §7`). The test control API (`/fake/*`) is only mounted when asked.
#![forbid(unsafe_code)]

pub mod connector;
pub mod control;
pub mod platform;

pub use connector::FakeConnector;
