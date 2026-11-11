//! Capability tokens: `prefix + base64url_nopad(32 random bytes)`.

use crate::secret::Secret;
use aws_lc_rs::{digest, rand};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use zeroize::Zeroize;

/// Length of the display prefix stored next to the hash.
pub const DISPLAY_PREFIX_LEN: usize = 12;
/// Number of random bytes in a token body.
pub const TOKEN_RANDOM_BYTES: usize = 32;

/// All token prefixes from `00 §Token prefixes and identifiers`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenPrefix {
    /// BYO agent gateway token.
    Gateway,
    /// Hosted sandbox boot token.
    Boot,
    /// Context credential.
    Context,
    /// Workspace API key.
    WorkspaceKey,
    /// Organization API key.
    OrgKey,
    /// Runner anchor token.
    Runner,
    /// Connector anchor token.
    Connector,
    /// SSH terminator anchor token.
    SshTerminator,
    /// Webhook capability token.
    Webhook,
    /// Session cookie value.
    Session,
    /// Invitation / email-link / account-link token.
    Invitation,
    /// Supervisor channel bootstrap token.
    Supervisor,
    /// Connector host anchor token (P4-MCON).
    ConnectorHost,
    /// SCIM provisioning token (P4-GRP-2).
    Scim,
}

impl TokenPrefix {
    pub const ALL: [TokenPrefix; 14] = [
        Self::Gateway,
        Self::Boot,
        Self::Context,
        Self::WorkspaceKey,
        Self::OrgKey,
        Self::Runner,
        Self::Connector,
        Self::SshTerminator,
        Self::Webhook,
        Self::Session,
        Self::Invitation,
        Self::Supervisor,
        Self::ConnectorHost,
        Self::Scim,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gateway => "aug_",
            Self::Boot => "aub_",
            Self::Context => "auc_",
            Self::WorkspaceKey => "auk_",
            Self::OrgKey => "auo_",
            Self::Runner => "aur_",
            Self::Connector => "aun_",
            Self::SshTerminator => "aut_",
            Self::Webhook => "auw_",
            Self::Session => "aus_",
            Self::Invitation => "aui_",
            Self::Supervisor => "aup_",
            Self::ConnectorHost => "auh_",
            Self::Scim => "aud_",
        }
    }
}

/// Generate a new token. The OS CSPRNG failing is unrecoverable, so this
/// panics in that (practically impossible) case rather than returning a
/// weak token.
#[allow(clippy::expect_used)]
pub fn generate(prefix: TokenPrefix) -> Secret<String> {
    let mut bytes = [0u8; TOKEN_RANDOM_BYTES];
    rand::fill(&mut bytes).expect("OS CSPRNG unavailable");
    let mut token = String::with_capacity(prefix.as_str().len() + 43);
    token.push_str(prefix.as_str());
    URL_SAFE_NO_PAD.encode_string(bytes, &mut token);
    bytes.zeroize();
    Secret::new(token)
}

/// A generated connector setting (P4-MCON, 13 §3.1): 32 random bytes from the OS RNG, standard
/// base64. Panics if the CSPRNG fails, like [`generate`].
#[allow(clippy::expect_used)]
pub fn generate_setting() -> Secret<String> {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes).expect("OS CSPRNG unavailable");
    let s = base64::engine::general_purpose::STANDARD.encode(bytes);
    bytes.zeroize();
    Secret::new(s)
}

/// SHA-256 of the full token string (what is stored, as `bytea`).
pub fn hash(token: &str) -> [u8; 32] {
    let d = digest::digest(&digest::SHA256, token.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(d.as_ref());
    out
}

/// First 12 characters (prefix + 8), stored for display.
pub fn display_prefix(token: &str) -> String {
    token.chars().take(DISPLAY_PREFIX_LEN).collect()
}

/// Identify the prefix of a presented token, if it is a known one.
pub fn parse_prefix(token: &str) -> Option<TokenPrefix> {
    TokenPrefix::ALL
        .into_iter()
        .find(|p| token.starts_with(p.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_are_distinct_four_chars() {
        let mut seen = std::collections::HashSet::new();
        for p in TokenPrefix::ALL {
            assert_eq!(p.as_str().len(), 4);
            assert!(p.as_str().starts_with("au") && p.as_str().ends_with('_'));
            assert!(seen.insert(p.as_str()));
        }
    }

    #[test]
    fn generate_shape_and_roundtrip() {
        for p in TokenPrefix::ALL {
            let t = generate(p);
            let s = t.expose();
            assert_eq!(s.len(), 4 + 43);
            assert_eq!(parse_prefix(s), Some(p));
            assert!(
                s[4..]
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            );
            assert_eq!(display_prefix(s), s[..12]);
        }
    }

    #[test]
    fn generations_differ_and_debug_hides() {
        let a = generate(TokenPrefix::Gateway);
        let b = generate(TokenPrefix::Gateway);
        assert_ne!(a.expose(), b.expose());
        let dbg = format!("{a:?} {a}");
        assert!(!dbg.contains(a.expose().as_str()));
        assert!(dbg.contains("[redacted]"));
    }

    #[test]
    fn hash_deterministic_32_bytes() {
        let h = hash("abc");
        assert_eq!(h, hash("abc"));
        assert_ne!(h, hash("abd"));
        // SHA-256("abc")
        assert_eq!(h[..4], [0xba, 0x78, 0x16, 0xbf]);
    }

    #[test]
    fn parse_prefix_unknown() {
        assert_eq!(parse_prefix("xyz_abc"), None);
        assert_eq!(parse_prefix(""), None);
        assert_eq!(parse_prefix("aug_"), Some(TokenPrefix::Gateway));
    }
}
