//! (P5-APPSKILL-1, C-1130, D-453) The one skill validator. The pure rules of a skill — name and
//! reserved prefix, description, `SKILL.md` frontmatter, file path grammar and depth, body, file
//! and total sizes, file count — live here and nowhere else: the package linter, the CLI and the
//! API (library reader, manual skills) all call it. Everything it sees is untrusted text; it takes
//! plain inputs, does no I/O and returns fixed, sanitized messages.
//!
//! Callers keep their own error shapes (a lint `Violation`, an `ApiError`, a library problem
//! string); the *rules* are shared.

use aurora_core::limits;
use aurora_core::sanitize::{is_stripped, one_line};
use base64::Engine as _;
use std::borrow::Cow;

/// More files than this in one skill directory is refused (bounds the blob reads of an add).
pub const FILES_PER_SKILL_MAX: usize = 50;
/// Folder depth of a skill file path (segments, the file name included).
pub const PATH_MAX_DEPTH: usize = 4;
/// Characters of a skill file path.
pub const PATH_MAX_LEN: usize = 200;

/// Why a name is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    /// Not `[a-z0-9]+(-[a-z0-9]+)*` of at most `SKILL_NAME_MAX` characters.
    Grammar,
    /// `aurora` or `aurora-…`: reserved for the platform.
    Reserved,
}

/// The name grammar and the reserved prefix.
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

/// A file path inside a skill: `[A-Za-z0-9._/-]`, at most 200 characters, depth at most 4, no empty,
/// `.` or `..` segment, and `SKILL.md` (any case) in no segment (FIX-110).
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

/// Characters for text, base64 length for binary content: the budget unit of a skill.
#[must_use]
pub fn content_size(c: &[u8]) -> usize {
    match std::str::from_utf8(c) {
        Ok(s) => s.chars().count(),
        Err(_) => base64::engine::general_purpose::STANDARD.encode(c).len(),
    }
}

/// Packaged skill files are UTF-8 text: valid UTF-8, no NUL, no byte-order mark (C-1130): the
/// home projection sends text only.
#[must_use]
pub fn is_text(bytes: &[u8]) -> bool {
    match std::str::from_utf8(bytes) {
        Ok(s) => !s.contains('\0') && !s.starts_with('\u{feff}'),
        Err(_) => false,
    }
}

/// Control and format characters (bidi, zero-width, …) are removed from repository text before it
/// can become part of what an agent is told; CRLF becomes LF. Everything else, including
/// indentation, is kept exactly.
#[must_use]
pub fn clean_text(s: &str) -> String {
    s.replace("\r\n", "\n")
        .chars()
        .filter(|c| !is_stripped(*c))
        .collect()
}

/// The two keys that matter; every other key is ignored (and, with anchors refused, bounded by the
/// file size).
#[derive(serde::Deserialize)]
struct Frontmatter {
    name: Option<serde_yaml::Value>,
    description: Option<serde_yaml::Value>,
}

/// Whether any YAML token starts with `&` (anchor) or `*` (alias); the shared pre-check
/// ([`aurora_core::yaml_guard`]) with this format's two indicators.
#[must_use]
pub fn has_anchor_or_alias(fm: &str) -> bool {
    aurora_core::yaml_guard::has_indicator_token(fm, &['&', '*'])
}

/// Which rule a skill broke; the linter maps it to its `app_skill_*` code, the API ignores it and
/// shows the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `SKILL.md` frontmatter or description.
    Frontmatter,
    /// A description longer than `SKILL_DESCRIPTION_MAX`.
    DescriptionTooLong,
    /// Body longer than `SKILL_BODY_MAX`.
    BodyTooLarge,
    /// A file path outside the grammar, or a path twice.
    Path,
    /// More than [`FILES_PER_SKILL_MAX`] files.
    TooManyFiles,
    /// A file larger than the per-file limit.
    FileTooLarge,
    /// Body and files over `SKILL_TOTAL_MAX`.
    TooLarge,
}

/// A broken skill rule with its fixed, sanitized message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillError {
    pub kind: Kind,
    pub message: String,
}

fn err<T>(kind: Kind, message: impl Into<String>) -> Result<T, SkillError> {
    Err(SkillError {
        kind,
        message: message.into(),
    })
}

/// [`parse_skill_md_typed`] with the message only.
pub fn parse_skill_md(raw: &[u8], dir: &str) -> Result<(String, String), String> {
    parse_skill_md_typed(raw, dir).map_err(|e| e.message)
}

/// `(description, body)` of a `SKILL.md`: YAML frontmatter with a single-line `description`
/// (and optionally a `name` equal to the directory name), then the body. Failures are fixed,
/// sanitized texts.
pub fn parse_skill_md_typed(raw: &[u8], dir: &str) -> Result<(String, String), SkillError> {
    fn bad_e(m: &str) -> SkillError {
        SkillError {
            kind: Kind::Frontmatter,
            message: m.to_owned(),
        }
    }
    fn bad<T>(m: &str) -> Result<T, SkillError> {
        Err(bad_e(m))
    }
    let text = std::str::from_utf8(raw).map_err(|_| bad_e("SKILL.md is not valid UTF-8"))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let text = text.replace("\r\n", "\n");
    let Some(rest) = text.strip_prefix("---\n") else {
        return bad("SKILL.md needs YAML frontmatter with a description");
    };
    let mut offset = 0;
    let mut end = None;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches('\n') == "---" {
            end = Some((offset, offset + line.len()));
            break;
        }
        offset += line.len();
    }
    let Some((fm_end, body_start)) = end else {
        return bad("SKILL.md frontmatter is not closed with ---");
    };
    // (FIX-096) Anchors and aliases are refused before the parser sees them: nested aliases
    // multiply (the parser bounds jumps, not the product), and repository text is untrusted.
    if has_anchor_or_alias(&rest[..fm_end]) {
        return bad("SKILL.md frontmatter must not use YAML anchors or aliases");
    }
    let fm: Frontmatter = match serde_yaml::from_str::<serde_yaml::Value>(&rest[..fm_end]) {
        Ok(serde_yaml::Value::Mapping(m)) => serde_yaml::from_value(serde_yaml::Value::Mapping(m))
            .map_err(|_| bad_e("SKILL.md frontmatter is not valid YAML"))?,
        Ok(_) => return bad("SKILL.md frontmatter must be a mapping"),
        Err(_) => return bad("SKILL.md frontmatter is not valid YAML"),
    };
    if let Some(n) = &fm.name {
        match n.as_str() {
            Some(n) if n == dir => {}
            _ => return bad("the frontmatter name must equal the directory name"),
        }
    }
    let desc = fm
        .description
        .as_ref()
        .and_then(serde_yaml::Value::as_str)
        .ok_or_else(|| bad_e("SKILL.md frontmatter needs a description"))?;
    let desc = clean_text(desc);
    if desc.trim().is_empty() {
        return bad("the description must not be empty");
    }
    if desc.chars().count() > limits::SKILL_DESCRIPTION_MAX {
        return err(
            Kind::DescriptionTooLong,
            format!(
                "the description is longer than {} characters",
                limits::SKILL_DESCRIPTION_MAX
            ),
        );
    }
    // A description is one line in the projected frontmatter.
    let description = one_line(&desc, limits::SKILL_DESCRIPTION_MAX);
    let body = clean_text(&rest[body_start..]);
    let body = body.trim_start_matches('\n').to_owned();
    if body.chars().count() > limits::SKILL_BODY_MAX {
        return err(
            Kind::BodyTooLarge,
            format!(
                "the body is longer than {} characters",
                limits::SKILL_BODY_MAX
            ),
        );
    }
    Ok((description, body))
}

/// The length the descriptions of one skill index are cut to (see
/// [`limits::SKILL_INDEX_SHARE`]), from their lengths in characters: the largest common cap
/// with which they take at most `SKILL_INDEX_SHARE` each together.
#[must_use]
pub fn index_cap(lengths: &[usize]) -> usize {
    let mut sorted = lengths.to_vec();
    sorted.sort_unstable();
    let mut room = sorted.len() * limits::SKILL_INDEX_SHARE;
    for (i, len) in sorted.iter().enumerate() {
        let share = room / (sorted.len() - i);
        if *len > share {
            return share;
        }
        room -= len;
    }
    limits::SKILL_DESCRIPTION_MAX
}

/// `description` as the index lists it: cut to `cap` characters, ending in "…", when longer.
#[must_use]
pub fn fold_description(description: &str, cap: usize) -> Cow<'_, str> {
    if description.chars().count() <= cap {
        return Cow::Borrowed(description);
    }
    let kept: String = description.chars().take(cap.saturating_sub(1)).collect();
    Cow::Owned(format!("{}…", kept.trim_end()))
}

/// The `SKILL.md` projected into an agent's home for a stored skill.
#[must_use]
pub fn projected_skill_md(name: &str, description: &str, body: &str) -> String {
    let description = serde_json::to_string(description).unwrap_or_else(|_| "\"\"".into());
    format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}")
}

/// Whether `text` fits one home-sync entry (`SKILL_ENTRY_JSON_MAX`, measured as a JSON
/// string); `what` names it in the message.
pub fn check_entry(what: &str, text: &str) -> Result<(), SkillError> {
    let fits = serde_json::to_string(text).is_ok_and(|j| j.len() <= limits::SKILL_ENTRY_JSON_MAX);
    if fits {
        return Ok(());
    }
    err(
        Kind::FileTooLarge,
        format!(
            "{what} is too large to send to an agent: written as JSON it may take at most {} \
             bytes, and line breaks, quotes and control characters take extra space",
            limits::SKILL_ENTRY_JSON_MAX
        ),
    )
}

/// [`check_entry`] for a skill's file. A file that is not UTF-8 travels as base64, which is
/// held to the same entry size.
pub fn check_file_entry(path: &str, content: &[u8]) -> Result<(), SkillError> {
    if let Ok(text) = std::str::from_utf8(content) {
        return check_entry(&format!("the file {path}"), text);
    }
    if content.len() <= limits::SKILL_BINARY_FILE_MAX {
        return Ok(());
    }
    err(
        Kind::FileTooLarge,
        format!(
            "the file {path} is too large to send to an agent: it isn't text, so it is sent as \
             base64, and a file that isn't text may be at most {} bytes",
            limits::SKILL_BINARY_FILE_MAX
        ),
    )
}

/// The checks on an already-parsed skill (description, body, files as `(path, content)`): what a
/// library approval re-runs on a stored proposal and what the linter runs on a package skill.
/// Returns the size in the budget unit (`body` characters plus [`content_size`] of the files).
/// `max_file` is the per-file byte limit.
pub fn check_content(
    name: &str,
    description: &str,
    body: &str,
    files: &[(&str, &[u8])],
    max_file: usize,
) -> Result<usize, SkillError> {
    if description.trim().is_empty() {
        return err(Kind::Frontmatter, "the description must not be empty");
    }
    if description.chars().count() > limits::SKILL_DESCRIPTION_MAX {
        return err(
            Kind::DescriptionTooLong,
            format!(
                "the description is longer than {} characters",
                limits::SKILL_DESCRIPTION_MAX
            ),
        );
    }
    if description.contains(['\n', '\r']) || clean_text(description) != description {
        return err(
            Kind::Frontmatter,
            "the description must be one line of plain text",
        );
    }
    if body.chars().count() > limits::SKILL_BODY_MAX {
        return err(
            Kind::BodyTooLarge,
            format!(
                "the body is longer than {} characters",
                limits::SKILL_BODY_MAX
            ),
        );
    }
    if clean_text(body) != body {
        return err(Kind::Frontmatter, "the body has control characters");
    }
    if files.len() > FILES_PER_SKILL_MAX {
        return err(
            Kind::TooManyFiles,
            format!("more than {FILES_PER_SKILL_MAX} files"),
        );
    }
    let mut seen = std::collections::BTreeSet::new();
    for (path, content) in files {
        if !path_ok(path) {
            return err(Kind::Path, format!("an unsupported file path ({path})"));
        }
        if !seen.insert(*path) {
            return err(Kind::Path, format!("a file appears twice ({path})"));
        }
        if content.len() > max_file {
            return err(
                Kind::FileTooLarge,
                format!("a file is larger than {max_file} bytes ({path})"),
            );
        }
    }
    let size = body.chars().count() + files.iter().map(|(_, c)| content_size(c)).sum::<usize>();
    if size > limits::SKILL_TOTAL_MAX {
        return err(
            Kind::TooLarge,
            format!(
                "the body and files total more than {} characters",
                limits::SKILL_TOTAL_MAX
            ),
        );
    }
    check_entry("SKILL.md", &projected_skill_md(name, description, body))?;
    for (path, content) in files {
        check_file_entry(path, content)?;
    }
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An anchor or alias on a continuation line of a flow collection is refused like one on
    /// the collection's first line.
    #[test]
    fn frontmatter_aliases_on_a_flow_continuation_line_are_refused() {
        for fm in [
            "description: d\nx: [a,\n  b, &z c]",
            "description: d\nx: &z c\ny: [a,\n  b, *z]",
            "description: d\nx: [a,\n  b, *z]",
        ] {
            let md = format!("---\n{fm}\n---\nbody\n");
            assert_eq!(
                parse_skill_md(md.as_bytes(), "s"),
                Err("SKILL.md frontmatter must not use YAML anchors or aliases".to_owned()),
                "{fm}"
            );
        }
    }

    #[test]
    fn the_index_cap_shares_the_room_and_never_cuts_a_short_description() {
        let share = limits::SKILL_INDEX_SHARE;
        // Room to spare: nothing is cut.
        assert_eq!(index_cap(&[]), limits::SKILL_DESCRIPTION_MAX);
        assert_eq!(index_cap(&[10, 10, 1_024]), limits::SKILL_DESCRIPTION_MAX);
        // All long: back to a share each.
        assert_eq!(index_cap(&[1_024, 1_024]), share);
        // The long ones split what the short ones leave.
        assert_eq!(index_cap(&[10, 1_024, 1_024]), (3 * share - 10) / 2);
        for lengths in [vec![500; 40], vec![1, 700, 800, 900, 1_024, 1_024]] {
            let cap = index_cap(&lengths);
            assert!(cap >= share, "{cap}");
            let total: usize = lengths.iter().map(|l| (*l).min(cap)).sum();
            assert!(total <= lengths.len() * share, "{total}");
        }
        assert_eq!(fold_description("abc", 3), "abc");
        assert_eq!(fold_description("ab cd", 4), "ab…");
        assert_eq!(fold_description("éééé", 3), "éé…");
    }

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
