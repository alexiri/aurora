use aurora_core::Secret;

use crate::{Result, SealContext, SecretError, SecretRef, StoreRegistry};

/// Mirrors `secrets.value_source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueSource {
    Managed,
    Reference,
}

/// The only place SecretRefs are interpreted.
pub struct SecretVault {
    registry: StoreRegistry,
}

impl SecretVault {
    pub fn new(registry: StoreRegistry) -> Self {
        Self { registry }
    }

    pub fn registry(&self) -> &StoreRegistry {
        &self.registry
    }

    fn store_for(&self, r: &SecretRef) -> Result<&dyn crate::SecretStore> {
        let id = r.store_id().unwrap_or("inline");
        self.registry
            .get(id)
            .map(AsRef::as_ref)
            .ok_or(SecretError::StoreNotConfigured)
    }

    /// Write a new value into the default store.
    pub async fn seal(&self, ctx: &SealContext, plaintext: Secret<Vec<u8>>) -> Result<SecretRef> {
        self.registry.default_store().put(ctx, &plaintext).await
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
            .get(target_store)
            .ok_or(SecretError::StoreNotConfigured)?;
        if !target.capabilities().managed {
            return Err(SecretError::ManagedNotSupported);
        }
        let value = self.open(ctx, old).await?;
        target.put(ctx, &value).await
    }
}
