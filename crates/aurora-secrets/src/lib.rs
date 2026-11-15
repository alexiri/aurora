//! aurora-secrets: SecretRef, the `SecretStore` seam, store registry and `SecretVault`
//! (docs/08-secrets-credentials.md §1–§3).
#![forbid(unsafe_code)]

mod dial_guard;
mod error;
mod hashicorp;
mod inline;
#[cfg(feature = "memory-store")]
mod memory;
mod recipe;
mod reference;
mod registry;
mod store;
pub mod tls;
mod vault;

pub use dial_guard::{DialGuard, StoreGuard};
pub use error::{Result, SecretError};
pub use hashicorp::VaultStore;
pub use inline::InlineStore;
#[cfg(feature = "memory-store")]
pub use memory::MemoryStore;
pub use recipe::InjectionRecipe;
pub use reference::SecretRef;
pub use registry::{StoreFailure, StoreRegistry};
pub use store::{Purpose, SealContext, SecretStore, StoreCapabilities, StoreHealth};
pub use vault::{SecretVault, ValueSource};
