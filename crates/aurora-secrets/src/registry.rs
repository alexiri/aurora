use std::collections::HashMap;
use std::sync::Arc;

use aurora_config::{SecretStoreConfig, SecretsSection};
use aurora_crypto::Keyring;

use crate::{InlineStore, Result, SecretError, SecretStore};

/// A store that could not be built when the registry was rebuilt at run time (FIX-124): it stays
/// in the dashboard list with its error and is never the default.
#[derive(Debug, Clone)]
pub struct StoreFailure {
    pub id: String,
    pub error: String,
}

/// Stores built from `[secrets]` configuration.
pub struct StoreRegistry {
    stores: HashMap<String, Arc<dyn SecretStore>>,
    default_store: String,
}

fn build_store(
    i: usize,
    s: &SecretStoreConfig,
    opener: &InlineStore,
    extra_ca_bundle: Option<&str>,
) -> Result<Arc<dyn SecretStore>> {
    if s.id.is_empty() || s.id.contains(':') {
        return Err(SecretError::InvalidConfig(
            "secrets.stores[].id must be non-empty and contain no ':'".into(),
        ));
    }
    Ok(match s.kind.as_str() {
        "inline" => Arc::new(InlineStore::new(&s.id, opener.keyring_arc())),
        aurora_config::VAULT_KIND => {
            let settings = aurora_config::VaultSettings::parse(s).map_err(|errs| {
                SecretError::InvalidConfig(
                    errs.iter()
                        .map(|(k, m)| {
                            if k.is_empty() {
                                format!("secrets.stores[{i}]: {m}")
                            } else {
                                format!("secrets.stores[{i}].{k}: {m}")
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("; "),
                )
            })?;
            Arc::new(crate::VaultStore::new_with(
                settings,
                extra_ca_bundle,
                Some(opener),
            )?)
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
    })
}

impl StoreRegistry {
    /// `inline` is implicit. Unknown or not-yet-available kinds fail startup.
    pub fn from_config(cfg: &SecretsSection, keyring: Arc<Keyring>) -> Result<Self> {
        Self::from_config_with_tls(cfg, keyring, None)
    }

    /// Like [`Self::from_config`]; `extra_ca_bundle` is `tls.extra_ca_bundle` (extra trust
    /// anchors for outbound TLS, e.g. an organization CA in front of Vault).
    pub fn from_config_with_tls(
        cfg: &SecretsSection,
        keyring: Arc<Keyring>,
        extra_ca_bundle: Option<&str>,
    ) -> Result<Self> {
        let opener = InlineStore::new("inline", keyring.clone());
        let mut stores: HashMap<String, Arc<dyn SecretStore>> = HashMap::new();
        for (i, s) in cfg.stores.iter().enumerate() {
            let store = build_store(i, s, &opener, extra_ca_bundle)?;
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

    /// Build one store from its settings entry, for Test connection (FIX-124): nothing is
    /// registered anywhere.
    pub fn build_single(
        cfg: &SecretStoreConfig,
        keyring: Arc<Keyring>,
        extra_ca_bundle: Option<&str>,
    ) -> Result<Arc<dyn SecretStore>> {
        let opener = InlineStore::new("inline", keyring);
        build_store(0, cfg, &opener, extra_ca_bundle)
    }

    /// Rebuild at run time from the merged settings (FIX-124): a store that fails to initialise
    /// is left out and reported; when the configured default is one of them (or is not managed)
    /// the default falls back to `inline`, so a failed store is never the default.
    pub fn from_config_tolerant(
        cfg: &SecretsSection,
        keyring: Arc<Keyring>,
        extra_ca_bundle: Option<&str>,
    ) -> (Self, Vec<StoreFailure>) {
        let opener = InlineStore::new("inline", keyring.clone());
        let mut stores: HashMap<String, Arc<dyn SecretStore>> = HashMap::new();
        let mut failed = Vec::new();
        for (i, s) in cfg.stores.iter().enumerate() {
            match build_store(i, s, &opener, extra_ca_bundle) {
                Ok(store) => {
                    if stores.insert(s.id.clone(), store).is_some() {
                        failed.push(StoreFailure {
                            id: s.id.clone(),
                            error: "duplicate secret store id".into(),
                        });
                    }
                }
                Err(e) => failed.push(StoreFailure {
                    id: s.id.clone(),
                    error: e.to_string(),
                }),
            }
        }
        stores
            .entry("inline".into())
            .or_insert_with(|| Arc::new(InlineStore::new("inline", keyring)));
        let mut reg = Self {
            stores,
            default_store: cfg.default_store.clone(),
        };
        if reg.check_default().is_err() {
            reg.default_store = "inline".into();
        }
        (reg, failed)
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

    pub fn get(&self, id: &str) -> Option<Arc<dyn SecretStore>> {
        self.stores.get(id).cloned()
    }

    pub fn default_store(&self) -> Arc<dyn SecretStore> {
        // Invariant upheld by `check_default`.
        self.stores[&self.default_store].clone()
    }

    /// The id of the default store.
    pub fn default_store_id(&self) -> &str {
        &self.default_store
    }

    /// Every store, in no particular order.
    pub fn stores(&self) -> impl Iterator<Item = &Arc<dyn SecretStore>> {
        self.stores.values()
    }

    /// True when some store supports references (dashboard form visibility).
    pub fn any_references(&self) -> bool {
        self.stores.values().any(|s| s.capabilities().references)
    }
}
