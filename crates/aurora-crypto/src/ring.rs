//! Keyring lists for the signing key and the internal secret (docs/08 §4, RFE-007, D-51).
//!
//! `<key_id>:<base64>,…` plus an active id; a bare base64 value (no `:`) is a one-entry
//! ring with id [`LEGACY_ID`], active. Producers use only the active entry; receivers
//! accept any entry.

use aurora_core::limits::KEYRING_MAX;
use aurora_core::secret::Secret;
use base64::Engine;
use base64::alphabet::STANDARD as B64_ALPHABET;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};

use crate::{CryptoError, Result};

/// Ring id of a bare (phase 0) value.
pub const LEGACY_ID: &str = "v0";

fn b64() -> GeneralPurpose {
    GeneralPurpose::new(
        &B64_ALPHABET,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
    )
}

pub fn valid_ring_id(id: &str) -> bool {
    (1..=16).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// What a ring's values must look like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingKind {
    /// Exactly 32 bytes (`AURORA_SIGNING_KEY`).
    Signing,
    /// At least 32 bytes (`AURORA_INTERNAL_SECRET`).
    Internal,
}

impl RingKind {
    fn check(self, len: usize) -> Result<()> {
        match self {
            RingKind::Signing if len != 32 => {
                Err(CryptoError::InvalidKeyring("key must decode to 32 bytes"))
            }
            RingKind::Internal if len < 32 => Err(CryptoError::InvalidKeyring(
                "secret must decode to at least 32 bytes",
            )),
            _ => Ok(()),
        }
    }
}

#[derive(Clone)]
pub struct RingEntry {
    id: String,
    value: Secret<Vec<u8>>,
}

impl RingEntry {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn value(&self) -> &[u8] {
        self.value.expose()
    }
}

/// An ordered, validated ring (1..=`KEYRING_MAX` entries) with one active entry.
#[derive(Clone, Default)]
pub struct SecretRing {
    entries: Vec<RingEntry>,
    active: usize,
}

impl std::fmt::Debug for SecretRing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretRing")
            .field("ids", &self.ids().collect::<Vec<_>>())
            .field("active", &self.active_id())
            .finish()
    }
}

impl SecretRing {
    /// Parse `raw` (the key list or a bare value) and the optional active id.
    pub fn parse(raw: &str, active: Option<&str>, kind: RingKind) -> Result<Self> {
        let raw = raw.trim();
        let active = active.map(str::trim).filter(|a| !a.is_empty());
        let mut entries: Vec<RingEntry> = Vec::new();
        if !raw.contains(':') {
            // Legacy bare value: ring `v0`, active.
            if active.is_some_and(|a| a != LEGACY_ID) {
                return Err(CryptoError::InvalidKeyring(
                    "active key id not in ring (a bare value is ring `v0`)",
                ));
            }
            entries.push(entry(LEGACY_ID, raw, kind)?);
            return Ok(Self { entries, active: 0 });
        }
        for item in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            let (id, v) = item
                .split_once(':')
                .ok_or(CryptoError::InvalidKeyring("entry must be <id>:<base64>"))?;
            if !valid_ring_id(id) {
                return Err(CryptoError::InvalidKeyring(
                    "key ids must match [a-z0-9]{1,16}",
                ));
            }
            if entries.iter().any(|e| e.id == id) {
                return Err(CryptoError::InvalidKeyring("duplicate key id"));
            }
            if entries.len() == KEYRING_MAX {
                return Err(CryptoError::InvalidKeyring("more than 4 entries"));
            }
            entries.push(entry(id, v, kind)?);
        }
        if entries.is_empty() {
            return Err(CryptoError::InvalidKeyring("no keys"));
        }
        let Some(active) = active else {
            return Err(CryptoError::InvalidKeyring("active key id is required"));
        };
        let idx = entries
            .iter()
            .position(|e| e.id == active)
            .ok_or(CryptoError::InvalidKeyring("active key id not in ring"))?;
        Ok(Self {
            entries,
            active: idx,
        })
    }

    /// A one-entry ring `v0` from raw bytes (tests, tools).
    pub fn single(value: Vec<u8>) -> Self {
        Self {
            entries: vec![RingEntry {
                id: LEGACY_ID.to_owned(),
                value: Secret::new(value),
            }],
            active: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[RingEntry] {
        &self.entries
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.id.as_str())
    }

    pub fn active(&self) -> Option<&RingEntry> {
        self.entries.get(self.active)
    }

    pub fn active_id(&self) -> &str {
        self.active().map_or("", RingEntry::id)
    }

    pub fn get(&self, id: &str) -> Option<&RingEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// The value producers present/sign with.
    pub fn active_value(&self) -> &[u8] {
        self.active().map_or(&[], RingEntry::value)
    }

    /// Short non-reversible fingerprint (8 hex chars of SHA-256) of an entry, for status output.
    pub fn fingerprint(entry: &RingEntry) -> String {
        use std::fmt::Write;
        let d = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, entry.value());
        let mut out = String::with_capacity(8);
        for b in &d.as_ref()[..4] {
            let _ = write!(out, "{b:02x}");
        }
        out
    }

    /// Constant-time check of a presented secret (raw bytes) against the entry named by
    /// `key_id`, or, with no id (legacy / rolling), against every entry.
    pub fn verify_presented(&self, key_id: Option<&str>, presented: &[u8]) -> bool {
        match key_id {
            Some(id) => self.get(id).is_some_and(|e| ct_eq(e.value(), presented)),
            None => {
                // No early exit: evaluate every entry.
                let mut ok = false;
                for e in &self.entries {
                    ok |= ct_eq(e.value(), presented);
                }
                ok
            }
        }
    }

    /// `(active id, [(id, fingerprint)])` for status output; never values.
    pub fn status(&self) -> (String, Vec<(String, String)>) {
        (
            self.active_id().to_owned(),
            self.entries
                .iter()
                .map(|e| (e.id.clone(), Self::fingerprint(e)))
                .collect(),
        )
    }

    /// Standard base64 of the active secret, as sent in `Authorization: Bearer`.
    pub fn active_bearer(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.active_value())
    }

    /// `Authorization` header value for an incoming request check: `Bearer <b64 of entry>`.
    pub fn verify_bearer_header(&self, key_id: Option<&str>, authorization: Option<&str>) -> bool {
        let Some(tok) = authorization.and_then(|h| h.strip_prefix("Bearer ")) else {
            return false;
        };
        let Ok(raw) = b64().decode(tok.trim()) else {
            return false;
        };
        let ok = self.verify_presented(key_id, &raw);
        ok && !self.is_empty()
    }
}

fn entry(id: &str, b64v: &str, kind: RingKind) -> Result<RingEntry> {
    let mut raw = b64()
        .decode(b64v.trim())
        .map_err(|_| CryptoError::InvalidKeyring("value is not valid base64"))?;
    if let Err(e) = kind.check(raw.len()) {
        zeroize::Zeroize::zeroize(&mut raw);
        return Err(e);
    }
    Ok(RingEntry {
        id: id.to_owned(),
        value: Secret::new(raw),
    })
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let mut d = u8::from(a.len() != b.len());
    for i in 0..a.len().max(b.len()) {
        d |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
    }
    d == 0
}
