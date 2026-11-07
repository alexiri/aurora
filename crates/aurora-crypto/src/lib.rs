//! aurora-crypto: at-rest envelope (`inline:v1`), HKDF-derived purpose keys, ed25519.
//! See docs/08-secrets-credentials.md §3-4.
#![forbid(unsafe_code)]

mod derived;
mod ed25519;
mod error;
mod inline;
mod ring;

pub use derived::{DerivedKeys, Purpose};
pub use ed25519::{
    SIGNATURE_LEN, generate_signing_key, parse_public_key_b64, parse_signature_b64, verify,
};
pub use error::CryptoError;
pub use inline::{Aad, Keyring};
pub use ring::{LEGACY_ID, RingEntry, RingKind, SecretRing, valid_ring_id};

/// Re-exported so callers can name ed25519 types without a direct dependency.
pub use ed25519_dalek::{Signature, SigningKey, VerifyingKey};

pub type Result<T, E = CryptoError> = std::result::Result<T, E>;

pub(crate) const NONCE_LEN: usize = 12;

use aurora_core::secret::Secret;
use aws_lc_rs::aead::{AES_256_GCM, Aad as AeadAad, LessSafeKey, Nonce, UnboundKey};
use aws_lc_rs::rand::fill;

/// AES-256-GCM seal; returns `nonce ‖ ciphertext ‖ tag`.
pub(crate) fn aead_seal(key: &[u8; 32], aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let mut nonce = [0u8; NONCE_LEN];
    fill(&mut nonce).map_err(|_| CryptoError::Rng)?;
    aead_seal_with_nonce(key, nonce, aad, plaintext)
}

pub(crate) fn aead_seal_with_nonce(
    key: &[u8; 32],
    nonce: [u8; NONCE_LEN],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    let k = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).map_err(|_| CryptoError::Cipher)?);
    let mut out = Vec::with_capacity(NONCE_LEN + plaintext.len() + 16);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(plaintext);
    let mut buf = out.split_off(NONCE_LEN);
    let tag = k
        .seal_in_place_separate_tag(
            Nonce::assume_unique_for_key(nonce),
            AeadAad::from(aad),
            &mut buf,
        )
        .map_err(|_| CryptoError::Cipher)?;
    out.extend_from_slice(&buf);
    out.extend_from_slice(tag.as_ref());
    Ok(out)
}

/// Inverse of [`aead_seal`]. All failures collapse to `Decrypt`.
pub(crate) fn aead_open(key: &[u8; 32], aad: &[u8], blob: &[u8]) -> Result<Secret<Vec<u8>>> {
    if blob.len() < NONCE_LEN + 16 {
        return Err(CryptoError::Malformed);
    }
    let (nonce, rest) = blob.split_at(NONCE_LEN);
    let mut nonce_arr = [0u8; NONCE_LEN];
    nonce_arr.copy_from_slice(nonce);
    let k = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).map_err(|_| CryptoError::Cipher)?);
    let mut buf = rest.to_vec();
    let n = match k.open_in_place(
        Nonce::assume_unique_for_key(nonce_arr),
        AeadAad::from(aad),
        &mut buf,
    ) {
        Ok(p) => p.len(),
        Err(_) => {
            zeroize::Zeroize::zeroize(&mut buf);
            return Err(CryptoError::Decrypt);
        }
    };
    buf.truncate(n);
    Ok(Secret::new(buf))
}
