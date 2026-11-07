/// Typed errors. Messages never include key material or plaintext.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CryptoError {
    #[error("invalid keyring: {0}")]
    InvalidKeyring(&'static str),
    #[error("unknown encryption key id")]
    UnknownKey,
    #[error("malformed sealed value")]
    Malformed,
    #[error("decryption failed")]
    Decrypt,
    #[error("cipher failure")]
    Cipher,
    #[error("random number generator failure")]
    Rng,
    #[error("key derivation failure")]
    Kdf,
    #[error("invalid base64")]
    Base64,
    #[error("invalid ed25519 key or signature")]
    InvalidKey,
    #[error("signature verification failed")]
    BadSignature,
}
