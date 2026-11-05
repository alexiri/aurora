//! aurora-config: layered TOML + env configuration, typed per section and
//! validated per service (`docs/15-deployment-operations.md`).
//!
//! ```ignore
//! let raw = aurora_config::load(None, std::env::vars())?;
//! let api = raw.api_config()?; // or validate_for(Service::Api)
//! ```
#![forbid(unsafe_code)]

mod error;
mod load;
mod raw;
mod redacted;
mod urlcheck;
mod validate;

pub use error::{ConfigError, ConfigErrors, Result};
pub use load::{CONFIG_PATH_ENV, DEFAULT_CONFIG_PATH, load};
pub use raw::*;
pub use redacted::Redacted;
pub use validate::*;
