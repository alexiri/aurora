//! aurora-connector-host: runs the built-in connector types for managed connectors
//! (`docs/13-channels-connectors.md` §3.1, SEC-69). The set of types is compiled in (crate
//! `aurora-connectors-builtin`); an assignment never selects code.
#![forbid(unsafe_code)]

pub mod api;
pub mod host;

/// The compiled-in types, their signing and the test-type key.
pub use aurora_connectors_builtin as catalogue;
pub use aurora_connectors_builtin::{Built, CompiledType, compiled, signing};
pub use host::{Host, HostConfig};
