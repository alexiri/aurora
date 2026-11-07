use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, SigningKey, VerifyingKey};

use crate::{CryptoError, Result};

pub const SIGNATURE_LEN: usize = 64;

/// Decode base64 (standard or url-safe, padded or not).
fn b64(s: &str) -> Result<Vec<u8>> {
    let t = s.trim().trim_end_matches('=');
    STANDARD
        .decode(s.trim())
        .or_else(|_| URL_SAFE_NO_PAD.decode(t))
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(t))
        .map_err(|_| CryptoError::Base64)
}

pub fn parse_public_key_b64(s: &str) -> Result<VerifyingKey> {
    let bytes: [u8; 32] = b64(s)?
        .as_slice()
        .try_into()
        .map_err(|_| CryptoError::InvalidKey)?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| CryptoError::InvalidKey)
}

pub fn parse_signature_b64(s: &str) -> Result<Signature> {
    let bytes: [u8; SIGNATURE_LEN] = b64(s)?
        .as_slice()
        .try_into()
        .map_err(|_| CryptoError::InvalidKey)?;
    Ok(Signature::from_bytes(&bytes))
}

/// Strict verification (rejects small-order keys / malleable signatures).
pub fn verify(public_key: &VerifyingKey, message: &[u8], signature: &Signature) -> Result<()> {
    public_key
        .verify_strict(message, signature)
        .map_err(|_| CryptoError::BadSignature)
}

/// Generate a fresh signing key (tests / CLI) from the aws-lc-rs CSPRNG.
pub fn generate_signing_key() -> Result<SigningKey> {
    let mut seed = [0u8; 32];
    aws_lc_rs::rand::fill(&mut seed).map_err(|_| CryptoError::Rng)?;
    let key = SigningKey::from_bytes(&seed);
    zeroize::Zeroize::zeroize(&mut seed);
    Ok(key)
}
