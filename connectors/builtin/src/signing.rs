//! Release signing of the built-in manifests (`aurora-connector-host sign-manifests`).
//!
//! Same scheme as connector `register` (13 §3): an ed25519 signature over
//! `sha256(canonical JSON of the manifest)` in the `aurora-sig-v1` text format, so the core
//! verifies it against the compiled-in project key (publisher `aurora`).

use std::path::Path;

use aurora_protocol::connector::ConnectorManifest;
use aurora_registry::builder::PackageBuilder;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};

/// `sha256(canonical JSON of the manifest)`: the bytes that are signed (and the approval digest).
pub fn digest(m: &ConnectorManifest) -> Result<[u8; 32], serde_json::Error> {
    let v = serde_json::to_value(m)?;
    Ok(Sha256::digest(aurora_registry::snapshot::canonical_bytes(&v)).into())
}

/// The key file written by `aurora plugin keygen`: the base64 of a 32-byte seed.
pub fn read_key(path: &Path) -> Result<SigningKey, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let raw = STANDARD
        .decode(text.trim())
        .map_err(|_| "key file is not base64".to_owned())?;
    let seed: [u8; 32] = raw
        .try_into()
        .map_err(|_| "key file must hold a 32-byte seed".to_owned())?;
    Ok(SigningKey::from_bytes(&seed))
}

/// The `aurora-sig-v1` signature text for `m`.
pub fn sign_manifest(key: &SigningKey, m: &ConnectorManifest) -> Result<String, String> {
    let d = digest(m).map_err(|e| e.to_string())?;
    Ok(PackageBuilder::sign_digest(
        key,
        &PackageBuilder::key_id(key),
        &d,
    ))
}

/// Writes `<type_id>.sig` for each production type into `out`. Returns the written type ids.
pub fn sign_all(key: &SigningKey, out: &Path) -> Result<Vec<String>, String> {
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut done = Vec::new();
    for t in crate::signable() {
        let sig = sign_manifest(key, &t.manifest)?;
        let file = out.join(format!("{}.sig", t.type_id));
        std::fs::write(&file, sig).map_err(|e| format!("{}: {e}", file.display()))?;
        done.push(t.type_id);
    }
    Ok(done)
}
