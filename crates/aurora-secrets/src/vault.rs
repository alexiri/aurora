use std::sync::Arc;

use arc_swap::ArcSwap;
use aurora_core::Secret;

use crate::{Result, SealContext, SecretError, SecretRef, StoreFailure, StoreRegistry};

/// Mirrors `secrets.value_source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueSource {
    Managed,
    Reference,
}

/// The only place SecretRefs are interpreted.
pub struct SecretVault {
    /// Swapped when the stores change in the dashboard (FIX-124): every call reads the current
    /// registry once, so an in-flight call finishes against the stores it started with.
    registry: ArcSwap<StoreRegistry>,
    failures: ArcSwap<Vec<StoreFailure>>,
}

impl SecretVault {
    pub fn new(registry: StoreRegistry) -> Self {
        Self {
            registry: ArcSwap::from_pointee(registry),
            failures: ArcSwap::from_pointee(Vec::new()),
        }
    }

    /// The current registry.
    pub fn registry(&self) -> Arc<StoreRegistry> {
        self.registry.load_full()
    }

    /// Replace the registry (live reload on a change of the `secrets` settings).
    pub fn swap_registry(&self, registry: StoreRegistry) {
        self.registry.store(Arc::new(registry));
        self.failures.store(Arc::new(Vec::new()));
    }

    /// Rebuild the registry from the merged `secrets` settings (FIX-124): a store that fails to
    /// initialise stays out of the registry and is reported by [`Self::failures`]; the default
    /// falls back to `inline` when it is one of them. Stores defined in the dashboard are
    /// dialled through the destination guard (SEC-89, SEC-90); Helm-defined ones are not.
    pub fn rebuild_from_effective(
        &self,
        eff: &aurora_config::Effective,
        keyring: Arc<aurora_crypto::Keyring>,
        extra_ca_bundle: Option<&str>,
    ) {
        let guard = crate::StoreGuard::for_dashboard_stores(
            eff,
            // `gateway.allowed_private` of the merged view opens private addresses to dashboard
            // Vault stores (SET-5: it is a live setting, so a rebuild reads the current list).
            &eff.raw.gateway.allowed_private,
        );
        let (reg, failed) = StoreRegistry::from_config_tolerant(
            &eff.raw.secrets,
            keyring,
            extra_ca_bundle,
            Some(&guard),
        );
        if let Some(configured) = reg.fell_back_from() {
            tracing::error!(
                store = configured,
                "the configured default secret store failed to start; new values go to the inline store until it is fixed"
            );
        }
        self.registry.store(Arc::new(reg));
        self.failures.store(Arc::new(failed));
    }

    /// Stores the last rebuild could not initialise, with the reason (never a credential).
    pub fn failures(&self) -> Arc<Vec<StoreFailure>> {
        self.failures.load_full()
    }

    fn store_for(&self, r: &SecretRef) -> Result<Arc<dyn crate::SecretStore>> {
        let id = r.store_id().unwrap_or("inline");
        self.registry
            .load()
            .get(id)
            .ok_or(SecretError::StoreNotConfigured)
    }

    /// Write a new value into the default store.
    pub async fn seal(&self, ctx: &SealContext, plaintext: Secret<Vec<u8>>) -> Result<SecretRef> {
        let store = self.registry.load().default_store();
        store.put(ctx, &plaintext).await
    }

    /// Write a new value into the **inline** store whatever the default store is (phase 5,
    /// SET-1, D-236(g), D-266(b)): instance-level secrets (sign-in, mail, push) must neither
    /// depend on an external store nor fan out to one.
    pub async fn seal_inline(
        &self,
        ctx: &SealContext,
        plaintext: Secret<Vec<u8>>,
    ) -> Result<SecretRef> {
        let store = self
            .registry
            .load()
            .get("inline")
            .ok_or(SecretError::StoreNotConfigured)?;
        store.put(ctx, &plaintext).await
    }

    pub async fn open(&self, ctx: &SealContext, r: &SecretRef) -> Result<Secret<Vec<u8>>> {
        self.store_for(r)?.get(ctx, r).await
    }

    /// No-op for inline refs and for reference-sourced values.
    pub async fn discard(
        &self,
        ctx: &SealContext,
        r: &SecretRef,
        source: ValueSource,
    ) -> Result<()> {
        if source == ValueSource::Reference || matches!(r, SecretRef::Inline { .. }) {
            return Ok(());
        }
        self.store_for(r)?.delete(ctx, r).await
    }

    /// Validate a user-supplied reference: store exists, supports references,
    /// path permitted, value readable.
    pub async fn check_reference(
        &self,
        ctx: &SealContext,
        store_id: &str,
        path: &str,
        field: Option<&str>,
    ) -> Result<SecretRef> {
        let store = self
            .registry
            .load()
            .get(store_id)
            .ok_or(SecretError::StoreNotConfigured)?;
        if !store.capabilities().references {
            return Err(SecretError::ReferencesNotSupported);
        }
        let r = store.resolve_reference(ctx, path, field).await?;
        store.get(ctx, &r).await?;
        Ok(r)
    }

    /// Migration helper: read `old` and write it into `target_store`
    /// (the caller performs the CAS swap and discards the old ref).
    pub async fn reseal(
        &self,
        ctx: &SealContext,
        old: &SecretRef,
        target_store: &str,
    ) -> Result<SecretRef> {
        let target = self
            .registry
            .load()
            .get(target_store)
            .ok_or(SecretError::StoreNotConfigured)?;
        if !target.capabilities().managed {
            return Err(SecretError::ManagedNotSupported);
        }
        let value = self.open(ctx, old).await?;
        target.put(ctx, &value).await
    }
}
