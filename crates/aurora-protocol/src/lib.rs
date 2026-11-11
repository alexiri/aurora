//! aurora-protocol: every machine-plane wire contract, plus JSON Schema
//! generation. See `docs/16-wire-protocols.md` and `docs/02-engineering-conventions.md`.
#![forbid(unsafe_code)]

#[macro_use]
mod tagged;

pub use tagged::UnknownMessage;

pub mod common;
pub mod connector;
pub mod gateway_internal;
pub mod hookhost;
pub mod host;
pub mod runner;
pub mod schema;
pub mod supervisor;
pub mod tools;
pub mod version;
pub mod vocab;

/// Re-exports used by macros in this crate so callers need no extra deps.
#[doc(hidden)]
pub mod __private {
    pub use schemars;
    pub use serde;
}
