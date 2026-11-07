use std::collections::HashMap;
use std::sync::Arc;

use aurora_config::SecretsSection;
use aurora_crypto::Keyring;

use crate::{InlineStore, Result, SecretError, SecretStore};

/// Stores built from `[secrets]` configuration.
pub struct StoreRegistry {
    stores: HashMap<String, Arc<dyn SecretStore>>,
    default_store: String,
}

impl StoreRegistry {
    /// `inline` is implicit. Unknown or not-yet-available kinds fail startup.
    pub fn from_config(cfg: &SecretsSection, keyring: Arc<Keyring>) -> Result<Self> {
        let mut stores: HashMap<String, Arc<dyn SecretStore>> = HashMap::new();
        for s in &cfg.stores {
            if s.id.is_empty() || s.id.contains(':') {
                return Err(SecretError::InvalidConfig(
                    "secrets.stores[].id must be non-empty and contain no ':'".into(),
                ));
            }
            let store: Arc<dyn SecretStore> = match s.kind.as_str() {
                "inline" => Arc::new(InlineStore::new(&s.id, keyring.clone())),
                "hashicorp-vault" => {
                    return Err(SecretError::InvalidConfig(format!(
                        "secret store '{}': kind 'hashicorp-vault' is available in phase 3",
                        s.id
                    )));
                }
                #[cfg(feature = "memory-store")]
                "memory" => Arc::new(crate::MemoryStore::new(&s.id)),
                #[cfg(not(feature = "memory-store"))]
                "memory" => {
                    return Err(SecretError::InvalidConfig(format!(
                        "secret store '{}': kind 'memory' is for tests only",
                        s.id
                    )));
                }
                other => {
                    return Err(SecretError::InvalidConfig(format!(
                        "secret store '{}': unknown kind '{other}'",
                        s.id
                    )));
                }
            };
            if stores.insert(s.id.clone(), store).is_some() {
                return Err(SecretError::InvalidConfig(format!(
                    "duplicate secret store id '{}'",
                    s.id
                )));
            }
        }
        if !stores.contains_key("inline") {
            stores.insert(
                "inline".into(),
                Arc::new(InlineStore::new("inline", keyring)),
            );
        }
        let reg = Self {
            stores,
            default_store: cfg.default_store.clone(),
        };
        reg.check_default()?;
        Ok(reg)
    }

    /// Add or replace a store (tests, or preloaded memory stores).
    pub fn register(&mut self, store: Arc<dyn SecretStore>) {
        self.stores.insert(store.id().to_owned(), store);
    }

    /// Change the default store; it must exist and be managed.
    pub fn set_default(&mut self, id: &str) -> Result<()> {
        id.clone_into(&mut self.default_store);
        self.check_default()
    }

    fn check_default(&self) -> Result<()> {
        match self.stores.get(&self.default_store) {
            None => Err(SecretError::InvalidConfig(format!(
                "secrets.default_store '{}' is not a configured store",
                self.default_store
            ))),
            Some(s) if !s.capabilities().managed => Err(SecretError::InvalidConfig(format!(
                "secrets.default_store '{}' must be a managed store",
                self.default_store
            ))),
            Some(_) => Ok(()),
        }
    }

    pub fn get(&self, id: &str) -> Option<&Arc<dyn SecretStore>> {
        self.stores.get(id)
    }

    pub fn default_store(&self) -> &Arc<dyn SecretStore> {
        // Invariant upheld by `check_default`.
        &self.stores[&self.default_store]
    }

    /// True when some store supports references (dashboard form visibility).
    pub fn any_references(&self) -> bool {
        self.stores.values().any(|s| s.capabilities().references)
    }
}
