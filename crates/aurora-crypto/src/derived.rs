use aurora_core::secret::Secret;
use aws_lc_rs::hkdf::{HKDF_SHA256, KeyType, Salt};
use aws_lc_rs::hmac;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use crate::{CryptoError, Result, aead_open, aead_seal};

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

/// Per-purpose keys derived from the signing root via HKDF-SHA256 with
/// info `aurora:<purpose>:v1`.
pub struct DerivedKeys {
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

impl DerivedKeys {
    pub fn new(signing_root: Secret<[u8; 32]>) -> Result<Self> {
        let [a, b, c, d, e] = Purpose::ALL;
        let root = signing_root.expose();
        Ok(Self {
            keys: [
                derive(root, a)?,
                derive(root, b)?,
                derive(root, c)?,
                derive(root, d)?,
                derive(root, e)?,
            ],
        })
    }

    fn key(&self, p: Purpose) -> &[u8; 32] {
        self.keys[p.index()].expose()
    }

    /// Returns `b64url(nonce ‖ ct ‖ tag)`.
    pub fn seal(&self, purpose: Purpose, aad: &[u8], plaintext: &[u8]) -> Result<String> {
        Ok(URL_SAFE_NO_PAD.encode(aead_seal(self.key(purpose), aad, plaintext)?))
    }

    pub fn open(&self, purpose: Purpose, aad: &[u8], sealed: &str) -> Result<Secret<Vec<u8>>> {
        let blob = URL_SAFE_NO_PAD
            .decode(sealed)
            .map_err(|_| CryptoError::Malformed)?;
        aead_open(self.key(purpose), aad, &blob)
    }

    /// HMAC-SHA256 tag.
    pub fn sign(&self, purpose: Purpose, data: &[u8]) -> Vec<u8> {
        let key = hmac::Key::new(hmac::HMAC_SHA256, self.key(purpose));
        hmac::sign(&key, data).as_ref().to_vec()
    }

    /// Constant-time verification.
    pub fn verify(&self, purpose: Purpose, data: &[u8], tag: &[u8]) -> bool {
        let key = hmac::Key::new(hmac::HMAC_SHA256, self.key(purpose));
        hmac::verify(&key, data, tag).is_ok()
    }
}
