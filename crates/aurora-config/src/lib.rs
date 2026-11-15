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
mod settings;
mod urlcheck;
mod validate;
mod vault_store;

pub use vault_store::{
    CredSource, MAX_CA_PEM_LEN, VAULT_KIND, VaultAuthSettings, VaultReferenceScope, VaultSettings,
    glob_matches, under_managed_prefix, valid_kv_path,
};

pub use error::{ConfigError, ConfigErrors, Result};
pub use load::{CONFIG_PATH_ENV, DEFAULT_CONFIG_PATH, load};
pub use raw::*;
pub use redacted::Redacted;
pub use settings::{
    ConfigSource, Effective, EffectiveKey, EntryView, ListKind, Loosening, NOT_SETTINGS, Registry,
    SECTION_IDS, SectionDef, SettingDef, SettingType, Source, Stored, TEST_SECTION, TEST_SECTION_B,
    dashboard_store_kind_allowed, registry, sections,
};
pub use validate::*;
