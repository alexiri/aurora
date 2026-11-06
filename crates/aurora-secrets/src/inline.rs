use std::sync::Arc;

use async_trait::async_trait;
use aurora_core::Secret;
use aurora_crypto::{Aad, CryptoError, Keyring};

use crate::{
    Result, SealContext, SecretError, SecretRef, SecretStore, StoreCapabilities, StoreHealth,
};

/// AES-256-GCM store: the ciphertext lives in the ref itself (08 §3).
pub struct InlineStore {
    id: String,
    keyring: Arc<Keyring>,
}

impl InlineStore {
    pub fn new(id: impl Into<String>, keyring: Arc<Keyring>) -> Self {
        Self {
            id: id.into(),
            keyring,
        }
    }

    pub fn keyring(&self) -> &Keyring {
        &self.keyring
    }

    fn aad(ctx: &SealContext) -> Aad<'static> {
        Aad {
            purpose: ctx.purpose.as_str(),
            org_id: ctx.org_id,
            owner_id: ctx.owner_id,
        }
    }
}

#[async_trait]
impl SecretStore for InlineStore {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> &'static str {
        "inline"
    }

    fn capabilities(&self) -> StoreCapabilities {
        StoreCapabilities {
            managed: true,
            references: false,
        }
    }

    async fn put(&self, ctx: &SealContext, plaintext: &Secret<Vec<u8>>) -> Result<SecretRef> {
        let sealed = self
            .keyring
            .seal(&Self::aad(ctx), plaintext.expose())
            .map_err(|_| SecretError::Internal)?;
        sealed.parse()
    }

    async fn get(&self, ctx: &SealContext, r: &SecretRef) -> Result<Secret<Vec<u8>>> {
        let sealed = r
            .sealed()
            .ok_or(SecretError::Malformed("not an inline reference"))?;
        self.keyring
            .open(&Self::aad(ctx), &sealed)
            .map_err(|e| match e {
                CryptoError::Decrypt => SecretError::SecretRefMismatch,
                CryptoError::UnknownKey => SecretError::Malformed("unknown encryption key id"),
                CryptoError::Malformed | CryptoError::Base64 => {
                    SecretError::Malformed("invalid inline ciphertext")
                }
                _ => SecretError::Internal,
            })
    }

    async fn delete(&self, _ctx: &SealContext, _r: &SecretRef) -> Result<()> {
        Ok(())
    }

    async fn resolve_reference(
        &self,
        _ctx: &SealContext,
        _path: &str,
        _field: Option<&str>,
    ) -> Result<SecretRef> {
        Err(SecretError::ReferencesNotSupported)
    }

    async fn health(&self) -> StoreHealth {
        StoreHealth::Healthy
    }
}
