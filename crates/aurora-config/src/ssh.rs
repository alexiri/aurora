//! SSH CA key material (phase 5, P4-SSH-1, D-239(d), C-797): parsing `AURORA_SSH_CA_KEY` and the
//! retired CA public keys. Used by startup validation and by the API's `local` signer, so the
//! two can never disagree about what a parsable key is. Error text never includes any part of
//! the key.
use base64::Engine;
use ssh_key::{Algorithm, PrivateKey, PublicKey, private::Ed25519Keypair};

/// The two accepted forms of the CA private key: `ed25519:<base64 of a 32-byte seed>` (what the
/// chart's `secrets` hook generates; it has no `ssh-keygen`) or an unencrypted OpenSSH
/// `-----BEGIN OPENSSH PRIVATE KEY-----` ed25519 key. Surrounding whitespace is ignored.
pub fn parse_ssh_ed25519_private_key(raw: &str) -> Result<PrivateKey, &'static str> {
    let raw = raw.trim();
    if let Some(b64) = raw.strip_prefix("ed25519:") {
        let seed = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|_| "the base64 after `ed25519:` does not decode")?;
        let seed: [u8; 32] = seed
            .try_into()
            .map_err(|_| "the seed after `ed25519:` must be exactly 32 bytes")?;
        return Ok(PrivateKey::from(Ed25519Keypair::from_seed(&seed)));
    }
    if raw.starts_with("-----BEGIN") {
        let key = PrivateKey::from_openssh(raw)
            .map_err(|_| "not a readable OpenSSH private key (is it passphrase-protected?)")?;
        if key.is_encrypted() {
            return Err(
                "the OpenSSH private key is passphrase-protected; an unencrypted key is required",
            );
        }
        if key.algorithm() != Algorithm::Ed25519 {
            return Err("the OpenSSH private key is not an ed25519 key");
        }
        return Ok(key);
    }
    Err("expected `ed25519:<base64 32-byte seed>` or an unencrypted OpenSSH ed25519 private key")
}

/// A retired CA public key line (`ssh-ed25519 <base64> [comment]`).
pub fn parse_ssh_ed25519_public_key(line: &str) -> Result<PublicKey, &'static str> {
    let key = PublicKey::from_openssh(line.trim())
        .map_err(|_| "not an OpenSSH public key line (`ssh-ed25519 <base64>`)")?;
    if key.algorithm() != Algorithm::Ed25519 {
        return Err("only ssh-ed25519 CA keys are supported");
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_form_round_trips_and_is_deterministic() {
        let seed = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        let a = parse_ssh_ed25519_private_key(&format!("ed25519:{seed}")).unwrap();
        let b = parse_ssh_ed25519_private_key(&format!("  ed25519:{seed}\n")).unwrap();
        assert_eq!(a.public_key(), b.public_key());
        assert_eq!(a.algorithm(), Algorithm::Ed25519);
    }

    #[test]
    fn garbage_is_refused_without_echoing_it() {
        for bad in [
            "",
            "ed25519:@@@",
            "ed25519:AAAA",
            "hunter2-secret-value",
            "-----BEGIN OPENSSH PRIVATE KEY-----\nnope\n-----END OPENSSH PRIVATE KEY-----",
        ] {
            let e = parse_ssh_ed25519_private_key(bad).unwrap_err();
            assert!(!e.contains("hunter2"), "{e}");
        }
    }

    #[test]
    fn public_line_must_be_ed25519() {
        let k = parse_ssh_ed25519_private_key(&format!(
            "ed25519:{}",
            base64::engine::general_purpose::STANDARD.encode([9u8; 32])
        ))
        .unwrap();
        let line = k.public_key().to_openssh().unwrap();
        assert!(parse_ssh_ed25519_public_key(&line).is_ok());
        assert!(parse_ssh_ed25519_public_key("ssh-rsa AAAA").is_err());
    }
}
