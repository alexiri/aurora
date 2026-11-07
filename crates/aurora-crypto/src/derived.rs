use aurora_core::secret::Secret;
use aws_lc_rs::hkdf::{HKDF_SHA256, KeyType, Salt};
use aws_lc_rs::hmac;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use crate::ring::LEGACY_ID;
use crate::{CryptoError, Result, SecretRing, aead_open, aead_seal};

/// HKDF purposes (docs/08 §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    OauthState,
    OidcState,
    ActionToken,
    AvatarKey,
    LinkState,
}

impl Purpose {
    pub const ALL: [Purpose; 5] = [
        Purpose::OauthState,
        Purpose::OidcState,
        Purpose::ActionToken,
        Purpose::AvatarKey,
        Purpose::LinkState,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Purpose::OauthState => "oauth-state",
            Purpose::OidcState => "oidc-state",
            Purpose::ActionToken => "action-token",
            Purpose::AvatarKey => "avatar-key",
            Purpose::LinkState => "link-state",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

struct Len32;
impl KeyType for Len32 {
    fn len(&self) -> usize {
        32
    }
}

/// Per-purpose keys derived from each signing-ring root via HKDF-SHA256 with
/// info `aurora:<purpose>:v1`. Seals and signs with the active ring entry; opens and
/// verifies with any entry (docs/08 §4, RFE-007).
pub struct DerivedKeys {
    ring: Vec<RingKeys>,
    active: usize,
}

struct RingKeys {
    id: String,
    keys: [Secret<[u8; 32]>; 5],
}

impl std::fmt::Debug for DerivedKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DerivedKeys([redacted])")
    }
}

fn derive(root: &[u8; 32], purpose: Purpose) -> Result<Secret<[u8; 32]>> {
    let info = format!("aurora:{}:v1", purpose.name());
    let prk = Salt::new(HKDF_SHA256, &[]).extract(root);
    let info_parts = [info.as_bytes()];
    let okm = prk
        .expand(&info_parts, Len32)
        .map_err(|_| CryptoError::Kdf)?;
    let mut out = [0u8; 32];
    okm.fill(&mut out).map_err(|_| CryptoError::Kdf)?;
    Ok(Secret::new(out))
}

fn derive_all(id: &str, root: &[u8; 32]) -> Result<RingKeys> {
    let [a, b, c, d, e] = Purpose::ALL;
    Ok(RingKeys {
        id: id.to_owned(),
        keys: [
            derive(root, a)?,
            derive(root, b)?,
            derive(root, c)?,
            derive(root, d)?,
            derive(root, e)?,
        ],
    })
}

impl DerivedKeys {
    /// One-entry ring `v0` (phase 0 shape; tests).
    pub fn new(signing_root: Secret<[u8; 32]>) -> Result<Self> {
        Ok(Self {
            ring: vec![derive_all(LEGACY_ID, signing_root.expose())?],
            active: 0,
        })
    }

    /// Derive from a signing ring (every entry must be 32 bytes).
    pub fn from_ring(ring: &SecretRing) -> Result<Self> {
        let mut out = Vec::new();
        let mut active = None;
        for e in ring.entries() {
            let root: [u8; 32] = e.value().try_into().map_err(|_| CryptoError::InvalidKey)?;
            if e.id() == ring.active_id() {
                active = Some(out.len());
            }
            out.push(derive_all(e.id(), &root)?);
        }
        Ok(Self {
            ring: out,
            active: active.ok_or(CryptoError::UnknownKey)?,
        })
    }

    /// Id of the key producers use.
    pub fn active_id(&self) -> &str {
        &self.ring[self.active].id
    }

    /// Ids of every ring entry.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.ring.iter().map(|r| r.id.as_str())
    }

    fn by_id(&self, id: &str) -> Option<&RingKeys> {
        self.ring.iter().find(|r| r.id == id)
    }

    fn key_of(r: &RingKeys, p: Purpose) -> &[u8; 32] {
        r.keys[p.index()].expose()
    }

    /// Returns `<key_id>.b64url(nonce ‖ ct ‖ tag)` sealed under the active key.
    pub fn seal(&self, purpose: Purpose, aad: &[u8], plaintext: &[u8]) -> Result<String> {
        let r = &self.ring[self.active];
        let blob = aead_seal(Self::key_of(r, purpose), aad, plaintext)?;
        Ok(format!("{}.{}", r.id, URL_SAFE_NO_PAD.encode(blob)))
    }

    /// Opens an id-prefixed value with the named key (an id outside the ring is refused),
    /// or an unprefixed (phase 0) value by trying each key.
    pub fn open(&self, purpose: Purpose, aad: &[u8], sealed: &str) -> Result<Secret<Vec<u8>>> {
        if let Some((id, rest)) = sealed.split_once('.') {
            let r = self.by_id(id).ok_or(CryptoError::UnknownKey)?;
            let blob = URL_SAFE_NO_PAD
                .decode(rest)
                .map_err(|_| CryptoError::Malformed)?;
            return aead_open(Self::key_of(r, purpose), aad, &blob);
        }
        let blob = URL_SAFE_NO_PAD
            .decode(sealed)
            .map_err(|_| CryptoError::Malformed)?;
        let mut last = CryptoError::Decrypt;
        for r in &self.ring {
            match aead_open(Self::key_of(r, purpose), aad, &blob) {
                Ok(v) => return Ok(v),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// HMAC-SHA256 tag under the active key.
    pub fn sign(&self, purpose: Purpose, data: &[u8]) -> Vec<u8> {
        let key = hmac::Key::new(
            hmac::HMAC_SHA256,
            Self::key_of(&self.ring[self.active], purpose),
        );
        hmac::sign(&key, data).as_ref().to_vec()
    }

    /// Constant-time verification against the key `id` only (false if `id` is outside the ring).
    pub fn verify_with(&self, id: &str, purpose: Purpose, data: &[u8], tag: &[u8]) -> bool {
        self.by_id(id).is_some_and(|r| {
            let key = hmac::Key::new(hmac::HMAC_SHA256, Self::key_of(r, purpose));
            hmac::verify(&key, data, tag).is_ok()
        })
    }

    /// Constant-time verification trying every ring key (artifacts without a key id).
    pub fn verify(&self, purpose: Purpose, data: &[u8], tag: &[u8]) -> bool {
        let mut ok = false;
        for r in &self.ring {
            let key = hmac::Key::new(hmac::HMAC_SHA256, Self::key_of(r, purpose));
            ok |= hmac::verify(&key, data, tag).is_ok();
        }
        ok
    }
}
