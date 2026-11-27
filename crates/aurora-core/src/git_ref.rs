//! The grammar of a git ref (branch, tag or commit name) the platform accepts, and the path
//! shape it shares with a repository name. The one implementation: the skill library
//! (`aurora_registry::library::check_ref`) and the template format's skill references
//! (`aurora-template`) both call it.

/// Longest ref accepted.
pub const REF_MAX: usize = 200;

/// `[A-Za-z0-9._/-]{1,max}`, no leading `-` or `/`, no trailing `/`, no empty, `.` or `..`
/// segment. The refusal is one fixed word: `length`, `characters`, `shape` or `segments`.
pub fn safe_segments(s: &str, max: usize) -> Result<(), &'static str> {
    if s.is_empty() || s.len() > max {
        return Err("length");
    }
    if !s
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'))
    {
        return Err("characters");
    }
    if s.starts_with('-') || s.starts_with('/') || s.ends_with('/') {
        return Err("shape");
    }
    if s.split('/')
        .any(|seg| seg.is_empty() || seg == "." || seg == "..")
    {
        return Err("segments");
    }
    Ok(())
}

/// A ref: [`safe_segments`] up to [`REF_MAX`], and no `..` anywhere.
pub fn check_ref(r: &str) -> Result<(), &'static str> {
    safe_segments(r, REF_MAX)?;
    if r.contains("..") {
        return Err("segments");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs() {
        for good in ["main", "v1.2.0", "feature/x_y-z", &"a".repeat(REF_MAX)] {
            assert_eq!(check_ref(good), Ok(()), "{good}");
        }
        for (bad, reason) in [
            ("", "length"),
            (&*"a".repeat(REF_MAX + 1), "length"),
            ("a b", "characters"),
            ("a|b", "characters"),
            ("a\u{202e}b", "characters"),
            ("-x", "shape"),
            ("/a", "shape"),
            ("a/", "shape"),
            ("a//b", "segments"),
            ("a/./b", "segments"),
            ("a..b", "segments"),
        ] {
            assert_eq!(check_ref(bad), Err(reason), "{bad}");
        }
    }
}
