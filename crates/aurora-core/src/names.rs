//! The one validator for every user-set name and label (groups, agents, secrets, app instances,
//! connections, schedules, webhooks, workspaces, organisations, API keys, plugin keys, …;
//! FIX-181 F6, D-396, C-1041).
//!
//! A name is trimmed, 1 to `max` characters, and contains no control character and no invisible
//! formatting character ([`crate::sanitize::is_stripped`]: zero-width, bidi controls, soft hyphen
//! …), so two people can never be shown names that differ only in what they cannot see. Rows that
//! already exist are not rewritten; editing one is refused until it is renamed.

use std::fmt;

use crate::sanitize::is_stripped;

/// Why a name was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    /// Empty after trimming, or longer than the given maximum.
    Length(usize),
    /// Contains a control or invisible formatting character.
    Forbidden,
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NameError::Length(max) => write!(f, "must be 1 to {max} characters"),
            NameError::Forbidden => {
                f.write_str("must not contain control or invisible formatting characters")
            }
        }
    }
}

impl std::error::Error for NameError {}

/// `true` for a character no name may contain: any control character or `is_stripped` one.
pub fn is_forbidden(c: char) -> bool {
    c.is_control() || is_stripped(c)
}

/// `true` when `s` contains a [`is_forbidden`] character.
pub fn has_forbidden(s: &str) -> bool {
    s.chars().any(is_forbidden)
}

/// The trimmed name when it is 1 to `max` characters and free of forbidden characters.
pub fn clean(s: &str, max: usize) -> Result<String, NameError> {
    let t = s.trim();
    let n = t.chars().count();
    if n == 0 || n > max {
        return Err(NameError::Length(max));
    }
    if has_forbidden(t) {
        return Err(NameError::Forbidden);
    }
    Ok(t.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_and_bounds() {
        assert_eq!(clean("  Ops  ", 80).unwrap(), "Ops");
        assert_eq!(clean("   ", 80), Err(NameError::Length(80)));
        assert_eq!(clean(&"x".repeat(81), 80), Err(NameError::Length(80)));
        assert!(clean(&"é".repeat(80), 80).is_ok());
    }

    #[test]
    fn rejects_control_and_invisible_formatting_characters() {
        for bad in [
            "a\u{200B}b", // zero-width space
            "a\u{202E}b", // right-to-left override
            "a\u{00AD}b", // soft hyphen
            "a\u{FEFF}b", // BOM
            "a\u{2060}b", // word joiner
            "a\nb",
            "a\u{0007}b",
        ] {
            assert_eq!(clean(bad, 80), Err(NameError::Forbidden), "{bad:?}");
        }
        assert_eq!(clean("Ünïcode ✓ name", 80).unwrap(), "Ünïcode ✓ name");
    }
}
