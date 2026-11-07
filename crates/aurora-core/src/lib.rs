//! aurora-core: dependency-light foundation shared by every Aurora crate.
//!
//! See `docs/02-engineering-conventions.md` (coding rules, error envelope,
//! identifiers and tokens, untrusted text, limits registry).
#![forbid(unsafe_code)]

pub mod error;
pub mod ids;
pub mod limits;
pub mod sanitize;
pub mod secret;
#[cfg(feature = "service")]
pub mod service;
#[cfg(feature = "service")]
pub mod telemetry;
pub mod token;

pub use error::{ApiError, FieldError};
pub use secret::Secret;
pub use token::TokenPrefix;
