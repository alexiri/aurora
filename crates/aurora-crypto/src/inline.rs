use std::collections::HashMap;

use aurora_core::secret::Secret;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use uuid::Uuid;

use crate::{CryptoError, Result, aead_open, aead_seal};

const PREFIX: &str = "inline:v1:";

/// Additional authenticated data binding a ciphertext to its owner (SEC-11).
#[derive(Debug, Clone, Copy)]
pub struct Aad<'a> {
    pub purpose: &'a str,
    pub org_id: Uuid,
    pub owner_id: Uuid,
}

impl Aad<'_> {
    /// `aurora:v1:<purpose>:<org_id>:<owner_id>`
    pub fn render(&self) -> String {
        format!(
            "aurora:v1:{}:{}:{}",
            self.purpose,
            self.org_id.hyphenated(),
            self.owner_id.hyphenated()
        )
    }
}

/// Set of encryption keys plus the active key id used for new seals.
pub struct Keyring {
    keys: HashMap<String, Secret<[u8; 32]>>,
    active: String,
}

impl std::fmt::Debug for Keyring {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keyring")
            .field("active", &self.active)
            .field("keys", &self.keys.len())
            .finish()
    }
}

fn valid_key_id(id: &str) -> bool {
    (1..=16).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

impl Keyring {
    /// Parse `<key_id>:<base64 32 bytes>,…` and the active key id.
    pub fn parse(keys: &str, active: &str) -> Result<Self> {
        let mut map = HashMap::new();
        for entry in keys.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            let (id, b64) = entry
                .split_once(':')
                .ok_or(CryptoError::InvalidKeyring("entry must be <id>:<base64>"))?;
            if !valid_key_id(id) {
                return Err(CryptoError::InvalidKeyring("invalid key id"));
            }
            let mut raw = STANDARD
                .decode(b64.trim())
                .map_err(|_| CryptoError::InvalidKeyring("key is not valid base64"))?;
            let arr: Result<[u8; 32], _> = raw.as_slice().try_into();
            zeroize::Zeroize::zeroize(&mut raw);
            let arr = arr.map_err(|_| CryptoError::InvalidKeyring("key must be 32 bytes"))?;
            if map.insert(id.to_owned(), Secret::new(arr)).is_some() {
                return Err(CryptoError::InvalidKeyring("duplicate key id"));
            }
        }
        if map.is_empty() {
            return Err(CryptoError::InvalidKeyring("no keys"));
        }
        if !map.contains_key(active) {
            return Err(CryptoError::InvalidKeyring("active key id not in keyring"));
        }
        Ok(Self {
            keys: map,
            active: active.to_owned(),
        })
    }

    pub fn active_key_id(&self) -> &str {
        &self.active
    }

    /// Seal under the active key with a fresh random nonce.
    pub fn seal(&self, aad: &Aad<'_>, plaintext: &[u8]) -> Result<String> {
        let key = self.keys.get(&self.active).ok_or(CryptoError::UnknownKey)?;
        let blob = aead_seal(key.expose(), aad.render().as_bytes(), plaintext)?;
        Ok(format!(
            "{PREFIX}{}:{}",
            self.active,
            URL_SAFE_NO_PAD.encode(blob)
        ))
    }

    pub fn open(&self, aad: &Aad<'_>, sealed: &str) -> Result<Secret<Vec<u8>>> {
        let (id, b64) = split(sealed)?;
        let key = self.keys.get(id).ok_or(CryptoError::UnknownKey)?;
        let blob = URL_SAFE_NO_PAD
            .decode(b64)
            .map_err(|_| CryptoError::Malformed)?;
        aead_open(key.expose(), aad.render().as_bytes(), &blob)
    }

    /// True when the value is sealed under a key other than the active one.
    /// Malformed values return false (they cannot be re-sealed without opening).
    pub fn needs_rekey(&self, sealed: &str) -> bool {
        split(sealed).is_ok_and(|(id, _)| id != self.active)
    }
}

fn split(sealed: &str) -> Result<(&str, &str)> {
    let rest = sealed.strip_prefix(PREFIX).ok_or(CryptoError::Malformed)?;
    let (id, b64) = rest.split_once(':').ok_or(CryptoError::Malformed)?;
    if !valid_key_id(id) {
        return Err(CryptoError::Malformed);
    }
    Ok((id, b64))
}
