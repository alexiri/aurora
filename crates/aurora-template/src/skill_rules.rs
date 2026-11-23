//! The skill rules a template's own skills must satisfy (`10 §Skills` and the library's file
//! rules, C-1141). The same grammar as `aurora-registry`'s skill format guard (the one skill
//! validator of C-1130); this crate may depend only on `aurora-core` (C-1104), so the small
//! pure checks are repeated here and pinned by tests. `aurora_core::limits` supplies the numbers.

use aurora_core::limits;

/// More files than this in one skill directory is refused.
pub const FILES_PER_SKILL_MAX: usize = 50;
/// Folder depth of a skill file path (segments, the file name included).
pub const PATH_MAX_DEPTH: usize = 4;
/// Characters of a skill file path.
pub const PATH_MAX_LEN: usize = 200;

/// Why a skill name is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    /// Not `[a-z0-9]+(-[a-z0-9]+)*` of at most `SKILL_NAME_MAX` characters.
    Grammar,
    /// `aurora` or `aurora-…`: reserved for the platform.
    Reserved,
}

/// The skill name grammar and the reserved prefix.
pub fn check_name(name: &str) -> Result<(), NameError> {
    let ok = !name.is_empty()
        && name.len() <= limits::SKILL_NAME_MAX
        && name.split('-').all(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        });
    if !ok {
        return Err(NameError::Grammar);
    }
    if name.starts_with("aurora-") || name == "aurora" {
        return Err(NameError::Reserved);
    }
    Ok(())
}

/// A file path inside a skill: `[A-Za-z0-9._/-]`, at most 200 characters, depth at most 4, no
/// empty, `.` or `..` segment, and `SKILL.md` (any case) in no segment (FIX-110).
#[must_use]
pub fn path_ok(path: &str) -> bool {
    let chars_ok = path
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'));
    let segs: Vec<&str> = path.split('/').collect();
    !path.is_empty()
        && path.len() <= PATH_MAX_LEN
        && chars_ok
        && segs.len() <= PATH_MAX_DEPTH
        && segs
            .iter()
            .all(|s| !s.is_empty() && *s != "." && *s != "..")
        && !segs.iter().any(|s| s.eq_ignore_ascii_case("skill.md"))
}

/// Packaged skill files must be UTF-8 text: valid UTF-8, no NUL, no byte-order mark.
#[must_use]
pub fn is_text(bytes: &[u8]) -> bool {
    match std::str::from_utf8(bytes) {
        Ok(s) => !s.contains('\0') && !s.starts_with('\u{feff}'),
        Err(_) => false,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(check_name("a-b2"), Ok(()));
        assert_eq!(check_name("A"), Err(NameError::Grammar));
        assert_eq!(check_name("a--b"), Err(NameError::Grammar));
        assert_eq!(check_name("aurora"), Err(NameError::Reserved));
        assert_eq!(check_name("aurora-x"), Err(NameError::Reserved));
        assert_eq!(check_name("auroral"), Ok(()));
    }

    #[test]
    fn paths() {
        assert!(path_ok("scripts/run.sh"));
        assert!(!path_ok("../x"));
        assert!(!path_ok("a/b/c/d/e"));
        assert!(!path_ok("Skill.md"));
        assert!(!path_ok("skill.MD/x"));
        assert!(!path_ok("a b"));
    }

    #[test]
    fn text() {
        assert!(is_text(b"hello\n"));
        assert!(!is_text(b"a\0b"));
        assert!(!is_text("\u{feff}x".as_bytes()));
        assert!(!is_text(&[0xff, 0xfe]));
    }
}
