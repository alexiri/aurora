//! aurora-hookhost: runs app-plugin hook components (docs/09 §8.3, docs/16 §W9).
//!
//! No database access and no encryption keys: the active registry snapshot and
//! the hook modules are pulled from the API's internal endpoint (D-23).
#![forbid(unsafe_code)]

pub mod fetch;
pub mod redact;
pub mod runtime;
pub mod server;
pub mod snapshot;
pub mod validate;
pub mod wire;

/// Highest snapshot `contract` this host understands.
pub const SUPPORTED_CONTRACT: u32 = 1;
