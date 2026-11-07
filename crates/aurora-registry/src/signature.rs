//! `SIGNATURE` file parsing and trust verification (09 §2).
use aurora_crypto::{Signature as Ed25519Sig, VerifyingKey};

use crate::Violation;
use crate::digest::digest;
use crate::package::Package;

pub const MAGIC: &str = "aurora-sig-v1";
pub const BUILTIN: &str = "builtin";
pub const PROJECT_PUBLISHER: &str = "aurora";

/// Parsed `SIGNATURE` file.
#[derive(Debug, Clone)]
pub struct Signature {
    pub key_id: String,
    pub signature: Ed25519Sig,
}

impl Signature {
    /// `aurora-sig-v1\n<key_id>\n<base64 signature>\n`
    pub fn parse(bytes: &[u8]) -> Result<Self, Violation> {
        let bad = |m: &str| Violation::new("signature_malformed", "SIGNATURE", m);
        let text = std::str::from_utf8(bytes).map_err(|_| bad("not UTF-8"))?;
        let mut lines = text.split('\n');
        let (Some(magic), Some(key_id), Some(sig), Some(""), None) = (
            lines.next(),
            lines.next(),
            lines.next(),
            lines.next(),
            lines.next(),
        ) else {
            return Err(bad("expected three newline-terminated lines"));
        };
        if magic != MAGIC {
            return Err(bad("unknown signature format"));
        }
        if key_id.is_empty() {
            return Err(bad("empty key id"));
        }
        let signature =
            aurora_crypto::parse_signature_b64(sig).map_err(|_| bad("bad signature encoding"))?;
        Ok(Self {
            key_id: key_id.to_owned(),
            signature,
        })
    }
}

#[derive(Debug, Clone)]
pub struct TrustedKey {
    pub publisher: String,
    pub key: VerifyingKey,
    pub revoked: bool,
}

#[derive(Debug, Clone, Default)]
pub struct TrustStore {
    /// Compiled-in project key (publisher `aurora`).
    pub project_key: Option<VerifyingKey>,
    pub keys: Vec<TrustedKey>,
}

/// Who vouched for the package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signer {
    /// Signed by the compiled-in project key.
    Project,
    /// Signed by a trusted, non-revoked key of this publisher.
    Trusted { publisher: String, key_id: String },
    /// No `SIGNATURE`, accepted only because `allow_unsigned` is set.
    Unsigned,
}

fn publisher_of(pkg: &Package) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct P {
        publisher: String,
    }
    #[derive(serde::Deserialize)]
    struct M {
        package: P,
    }
    serde_yaml::from_str::<M>(pkg.manifest_yaml()?)
        .ok()
        .map(|m| m.package.publisher)
}

fn v(code: &'static str, msg: impl Into<String>) -> Violation {
    Violation::new(code, "SIGNATURE", msg)
}

/// Verify the package signature against `trust`. Failure of the ed25519 check
/// over the canonical digest (e.g. a tampered file) is `digest_mismatch`.
pub fn verify(
    pkg: &Package,
    trust: &TrustStore,
    allow_unsigned: bool,
) -> Result<Signer, Violation> {
    let Some(raw) = &pkg.signature else {
        return if allow_unsigned {
            Ok(Signer::Unsigned)
        } else {
            Err(v("signature_missing", "package is not signed"))
        };
    };
    let publisher = publisher_of(pkg)
        .ok_or_else(|| v("publisher_mismatch", "manifest publisher unreadable"))?;
    verify_digest(raw, &publisher, &digest(pkg), trust)
}

/// Verify a `SIGNATURE`-format detached signature (`aurora-sig-v1\n<key_id>\n<b64>\n`) over
/// `digest` for `publisher`. Shared by packages and connector manifests (13 §3).
pub fn verify_digest(
    raw: &[u8],
    publisher: &str,
    d: &[u8; 32],
    trust: &TrustStore,
) -> Result<Signer, Violation> {
    let sig = Signature::parse(raw)?;
    let publisher = publisher.to_owned();
    let check = |key: &VerifyingKey| {
        aurora_crypto::verify(key, d, &sig.signature).map_err(|_| {
            v(
                "digest_mismatch",
                "signature does not match package contents",
            )
        })
    };

    let project = |key: &VerifyingKey| -> Result<Signer, Violation> {
        if publisher != PROJECT_PUBLISHER {
            return Err(v(
                "publisher_mismatch",
                format!("project key may only sign publisher `{PROJECT_PUBLISHER}`"),
            ));
        }
        check(key)?;
        Ok(Signer::Project)
    };

    if sig.key_id == BUILTIN {
        let key = trust
            .project_key
            .as_ref()
            .ok_or_else(|| v("signature_untrusted", "no project key configured"))?;
        return project(key);
    }
    let key = aurora_crypto::parse_public_key_b64(&sig.key_id)
        .map_err(|_| v("signature_malformed", "key id is not a public key"))?;
    if trust.project_key.as_ref() == Some(&key) {
        return project(&key);
    }
    let rows: Vec<&TrustedKey> = trust.keys.iter().filter(|k| k.key == key).collect();
    if rows.is_empty() {
        return Err(v("signature_untrusted", "signing key is not trusted"));
    }
    let same_pub: Vec<&&TrustedKey> = rows.iter().filter(|k| k.publisher == publisher).collect();
    if same_pub.is_empty() {
        return Err(v(
            "publisher_mismatch",
            format!("key is not trusted for publisher `{publisher}`"),
        ));
    }
    if same_pub.iter().all(|k| k.revoked) {
        return Err(v("key_revoked", "signing key has been revoked"));
    }
    check(&key)?;
    Ok(Signer::Trusted {
        publisher,
        key_id: sig.key_id,
    })
}
