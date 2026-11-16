//! The SSH CA signer seam (C-791, D-239(d)). The API signs user certificates; the CA private
//! key lives here only, behind [`SshCaSigner`] (`local` from `AURORA_SSH_CA_KEY` in this batch;
//! a KMS signer would be a second implementation). The key never appears in a response, a log
//! line, an error or the database: this type has no `Debug` output of the key and its errors
//! carry no key material.
use async_trait::async_trait;
use aurora_core::ApiError;
use ssh_key::certificate::{Builder, Certificate};
use ssh_key::{PrivateKey, PublicKey};

#[async_trait]
pub trait SshCaSigner: Send + Sync {
    /// The CA public key (what terminators trust and `known_hosts`-style tooling pins).
    fn public_key(&self) -> PublicKey;
    /// Sign a prepared certificate. The signer adds nothing to it: every field is set by the
    /// caller (the mint), so the CA cannot be steered by anything but the builder.
    async fn sign(&self, tbs: Builder) -> Result<Certificate, ApiError>;
}

/// The `local` signer: an ed25519 key held in memory from `AURORA_SSH_CA_KEY`.
pub struct LocalSigner {
    key: PrivateKey,
}

impl LocalSigner {
    /// Parse the configured key. Config validation already refused a missing or garbled key when
    /// `ssh.enabled`; the error text never includes any part of it.
    pub fn from_config(cfg: &aurora_config::ApiConfig) -> Result<Self, String> {
        let raw = cfg
            .ssh_ca_key
            .as_ref()
            .ok_or_else(|| "required when ssh.enabled is true".to_owned())?;
        let key =
            aurora_config::parse_ssh_ed25519_private_key(raw.expose()).map_err(str::to_owned)?;
        Ok(Self { key })
    }

    /// A signer over an explicit key (tests).
    pub fn new(key: PrivateKey) -> Self {
        Self { key }
    }
}

impl std::fmt::Debug for LocalSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalSigner").finish_non_exhaustive()
    }
}

#[async_trait]
impl SshCaSigner for LocalSigner {
    fn public_key(&self) -> PublicKey {
        self.key.public_key().clone()
    }

    async fn sign(&self, tbs: Builder) -> Result<Certificate, ApiError> {
        tbs.sign(&self.key).map_err(|e| {
            // The error type of the crate carries no key bytes; log its kind only.
            tracing::error!(error = %e, "ssh certificate signing failed");
            ApiError::unavailable("ssh_signer_unavailable", "The SSH CA could not sign.")
        })
    }
}
