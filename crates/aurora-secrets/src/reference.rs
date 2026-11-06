use std::fmt;
use std::str::FromStr;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use percent_encoding::{AsciiSet, CONTROLS, percent_decode_str, utf8_percent_encode};

use crate::{Result, SecretError};

const INLINE_PREFIX: &str = "inline:v1:";
/// Characters that must be escaped inside an `ext` path or field.
const ENC: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'%')
    .add(b'#')
    .add(b'@')
    .add(b':')
    .add(b'?')
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'`')
    .add(b'{')
    .add(b'}');

/// Parsed form of a value stored in a `*_ref` column.
///
/// `Display`/`Debug` are deliberately abbreviated; use [`SecretRef::to_column`]
/// for the full string that is persisted.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum SecretRef {
    /// `inline:v1:<key_id>:<b64url(nonce‖ciphertext‖tag)>`
    Inline { key_id: String, blob: String },
    /// `ext:<store_id>:<pct path>[#<pct field>]@<version>`
    Ext {
        store_id: String,
        path: String,
        field: Option<String>,
        version: String,
    },
}

fn valid_key_id(id: &str) -> bool {
    (1..=16).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

fn decode(s: &str) -> Result<String> {
    percent_decode_str(s)
        .decode_utf8()
        .map(std::borrow::Cow::into_owned)
        .map_err(|_| SecretError::Malformed("invalid percent-encoding"))
}

impl SecretRef {
    pub fn inline(key_id: &str, blob: &str) -> Result<Self> {
        let r = Self::Inline {
            key_id: key_id.to_owned(),
            blob: blob.to_owned(),
        };
        r.validate()?;
        Ok(r)
    }

    pub fn ext(store_id: &str, path: &str, field: Option<&str>, version: &str) -> Result<Self> {
        let r = Self::Ext {
            store_id: store_id.to_owned(),
            path: path.to_owned(),
            field: field.map(str::to_owned),
            version: version.to_owned(),
        };
        r.validate()?;
        Ok(r)
    }

    fn validate(&self) -> Result<()> {
        match self {
            Self::Inline { key_id, blob } => {
                if !valid_key_id(key_id) {
                    return Err(SecretError::Malformed("invalid key id"));
                }
                // nonce(12) + tag(16) minimum.
                match URL_SAFE_NO_PAD.decode(blob) {
                    Ok(b) if b.len() >= 28 => Ok(()),
                    _ => Err(SecretError::Malformed("invalid inline ciphertext")),
                }
            }
            Self::Ext {
                store_id,
                path,
                field,
                version,
            } => {
                if store_id.is_empty() || store_id.contains(':') {
                    return Err(SecretError::Malformed("invalid store id"));
                }
                if path.is_empty() {
                    return Err(SecretError::Malformed("empty path"));
                }
                if field.as_deref() == Some("") {
                    return Err(SecretError::Malformed("empty field"));
                }
                if version.is_empty() || version.contains('@') {
                    return Err(SecretError::Malformed("invalid version"));
                }
                Ok(())
            }
        }
    }

    /// Full canonical string persisted in the `*_ref` column.
    pub fn to_column(&self) -> String {
        match self {
            Self::Inline { key_id, blob } => format!("{INLINE_PREFIX}{key_id}:{blob}"),
            Self::Ext {
                store_id,
                path,
                field,
                version,
            } => {
                let mut s = format!("ext:{store_id}:{}", utf8_percent_encode(path, ENC));
                if let Some(f) = field {
                    s.push('#');
                    s.extend(utf8_percent_encode(f, ENC));
                }
                s.push('@');
                s.push_str(version);
                s
            }
        }
    }

    pub fn store_id(&self) -> Option<&str> {
        match self {
            Self::Inline { .. } => None,
            Self::Ext { store_id, .. } => Some(store_id),
        }
    }

    /// Sealed value in `aurora-crypto` format (inline refs only).
    pub(crate) fn sealed(&self) -> Option<String> {
        match self {
            Self::Inline { key_id, blob } => Some(format!("{INLINE_PREFIX}{key_id}:{blob}")),
            Self::Ext { .. } => None,
        }
    }
}

impl FromStr for SecretRef {
    type Err = SecretError;

    fn from_str(s: &str) -> Result<Self> {
        if let Some(rest) = s.strip_prefix(INLINE_PREFIX) {
            let (key_id, blob) = rest
                .split_once(':')
                .ok_or(SecretError::Malformed("inline ref missing ciphertext"))?;
            return Self::inline(key_id, blob);
        }
        if let Some(rest) = s.strip_prefix("ext:") {
            let (store_id, rest) = rest
                .split_once(':')
                .ok_or(SecretError::Malformed("ext ref missing path"))?;
            let (loc, version) = rest
                .rsplit_once('@')
                .ok_or(SecretError::Malformed("ext ref missing version"))?;
            let (path, field) = match loc.split_once('#') {
                Some((p, f)) => (decode(p)?, Some(decode(f)?)),
                None => (decode(loc)?, None),
            };
            return Self::ext(store_id, &path, field.as_deref(), version);
        }
        Err(SecretError::Malformed("unknown secret reference scheme"))
    }
}

impl fmt::Display for SecretRef {
    /// Abbreviated: ciphertext is cut to a short prefix.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inline { key_id, blob } => {
                let cut: String = blob.chars().take(6).collect();
                write!(f, "{INLINE_PREFIX}{key_id}:{cut}…")
            }
            Self::Ext { .. } => f.write_str(&self.to_column()),
        }
    }
}

impl fmt::Debug for SecretRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretRef({self})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_round_trip() {
        for (path, field) in [
            ("kv/team-ai/openai", Some("key")),
            ("a b/c#d@e:f%g", Some("fi#eld@x")),
            ("üñí/路径", None),
        ] {
            let r = SecretRef::ext("vault-prod", path, field, "7").unwrap();
            let s = r.to_column();
            assert_eq!(s.matches('@').count(), 1, "{s}");
            assert_eq!(s.parse::<SecretRef>().unwrap(), r);
        }
    }

    #[test]
    fn inline_round_trip_and_abbreviation() {
        let blob = URL_SAFE_NO_PAD.encode([7u8; 40]);
        let r = SecretRef::inline("k1", &blob).unwrap();
        let s = r.to_column();
        assert_eq!(s, format!("inline:v1:k1:{blob}"));
        assert_eq!(s.parse::<SecretRef>().unwrap(), r);
        assert!(!format!("{r}").contains(&blob));
        assert!(!format!("{r:?}").contains(&blob));
    }

    #[test]
    fn rejects_malformed() {
        for s in [
            "",
            "foo:bar",
            "inline:v1:K1:AAAA",
            "inline:v1:k1:AAAA",
            "inline:v1:k1",
            "ext:s:path",
            "ext::p@1",
            "ext:s:@1",
            "ext:s:p@",
            "ext:s:p#@1",
            "ext:s:%ff@1",
        ] {
            assert!(s.parse::<SecretRef>().is_err(), "{s}");
        }
    }
}
