//! Memory projection checksums and harvest of agent edits
//! (`docs/10-agents-conversations.md` §Memory, `docs/12-supervisor-harness.md` §Memory harvest).
//!
//! Every projected `memory/<key>.md` ends with `<!-- aurora:checksum sha256=<hex> -->`, the hash of
//! the whole file with the hex zeroed. A file that fails verification (or that is new) is an agent
//! edit: it is uploaded with `memory.write` and left in place until the canonical render replaces it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use aurora_core::limits::MEMORY_CONTENT_MAX;
use aurora_protocol::common::Validate;
use aurora_protocol::runner::MemoryWrite;
use sha2::{Digest, Sha256};

pub const TRAILER_PREFIX: &str = "<!-- aurora:checksum sha256=";
const TRAILER_SUFFIX: &str = " -->";
const ZEROS: &str = "0000000000000000000000000000000000000000000000000000000000000000";
pub const MEMORY_DIR: &str = "memory";
pub const INDEX_FILE: &str = "index.md";
const KEY_MAX: usize = 128;

fn sha_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Appends the checksum trailer to projected memory `content`.
pub fn seal(content: &str) -> String {
    let mut file = String::with_capacity(content.len() + 100);
    file.push_str(content);
    if !file.ends_with('\n') {
        file.push('\n');
    }
    file.push_str(TRAILER_PREFIX);
    file.push_str(ZEROS);
    file.push_str(TRAILER_SUFFIX);
    file.push('\n');
    let digest = sha_hex(file.as_bytes());
    let start = file.len() - 1 - TRAILER_SUFFIX.len() - 64;
    file.replace_range(start..start + 64, &digest);
    file
}

/// Splits a file into (body before the trailer, trailer hex) if a well-formed trailer ends it.
fn split_trailer(file: &str) -> Option<(usize, &str)> {
    let at = file.rfind(TRAILER_PREFIX)?;
    let rest = &file[at + TRAILER_PREFIX.len()..];
    let hexpart = rest.get(..64)?;
    let tail = &rest[64..];
    if !hexpart.bytes().all(|b| b.is_ascii_hexdigit())
        || !(tail == TRAILER_SUFFIX || tail == "\u{20}-->\n")
    {
        return None;
    }
    Some((at, hexpart))
}

/// True when the file still carries a trailer matching its bytes (an unmodified projection).
pub fn verify(file: &str) -> bool {
    let Some((at, hex)) = split_trailer(file) else {
        return false;
    };
    let start = at + TRAILER_PREFIX.len();
    let mut zeroed = file.to_owned();
    zeroed.replace_range(start..start + 64, ZEROS);
    sha_hex(zeroed.as_bytes()).eq_ignore_ascii_case(hex)
}

/// The file without its trailer (when it has one).
pub fn strip_trailer(file: &str) -> &str {
    split_trailer(file).map_or(file, |(at, _)| &file[..at])
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ParsedMemory {
    pub title: Option<String>,
    pub description: Option<String>,
    pub content: String,
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        // The API renders a double-quoted scalar with `serde_json::to_string`; read it back the
        // same way. A slightly malformed hand edit keeps its inner text rather than being lost.
        return serde_json::from_str::<String>(v).unwrap_or_else(|_| v[1..v.len() - 1].to_owned());
    }
    if v.len() >= 2 && v.starts_with('\'') && v.ends_with('\'') {
        // YAML single-quoted: the only escape is `''` for a literal `'`.
        return v[1..v.len() - 1].replace("''", "'");
    }
    v.to_owned()
}

/// Parses optional YAML front matter (`title:` / `description:` only) off a memory file.
pub fn parse_memory(file: &str) -> ParsedMemory {
    let body = strip_trailer(file);
    let mut out = ParsedMemory {
        content: body.to_owned(),
        ..ParsedMemory::default()
    };
    let Some(rest) = body.strip_prefix("---\n") else {
        return out;
    };
    let (front, after) = if let Some(i) = rest.find("\n---\n") {
        (&rest[..i], &rest[i + 5..])
    } else if let Some(front) = rest.strip_suffix("\n---") {
        (front, "")
    } else {
        return out;
    };
    for line in front.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let v = unquote(v);
            if v.is_empty() {
                continue;
            }
            match k.trim() {
                "title" => out.title = Some(v),
                "description" => out.description = Some(v),
                _ => {}
            }
        }
    }
    out.content = after.to_owned();
    out
}

/// Memory keys: lowercase ASCII words joined by hyphens.
pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= KEY_MAX
        && !key.starts_with('-')
        && !key.ends_with('-')
        && !key.contains("--")
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Scans `memory/*.md` for agent edits. Remembers what it already uploaded so unchanged edits
/// are not re-sent every tick.
#[derive(Debug)]
pub struct Harvester {
    dir: PathBuf,
    uploaded: HashMap<String, String>,
    notices: Vec<String>,
}

impl Harvester {
    pub fn new(root: &Path) -> Self {
        Self {
            dir: root.join(MEMORY_DIR),
            uploaded: HashMap::new(),
            notices: Vec::new(),
        }
    }

    /// Notices for the next turn (invalid keys, oversized files); drains them.
    pub fn take_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notices)
    }

    pub fn scan(&mut self) -> Vec<MemoryWrite> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut names: Vec<String> = rd
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.ends_with(".md") && n != INDEX_FILE)
            .collect();
        names.sort();
        let mut out = Vec::new();
        for name in names {
            let Ok(text) = std::fs::read_to_string(self.dir.join(&name)) else {
                continue;
            };
            if verify(&text) {
                self.uploaded.remove(&name);
                continue;
            }
            let digest = sha_hex(text.as_bytes());
            if self.uploaded.get(&name) == Some(&digest) {
                continue;
            }
            let key = name.trim_end_matches(".md");
            if !valid_key(key) {
                self.notice(format!(
                    "memory file `{name}` was not saved: keys are lowercase words joined by hyphens"
                ));
                self.uploaded.insert(name, digest);
                continue;
            }
            let p = parse_memory(&text);
            let write = MemoryWrite {
                key: key.to_owned(),
                title: p.title,
                description: p.description,
                content: p.content,
            };
            if write.content.chars().count() > MEMORY_CONTENT_MAX || write.validate().is_err() {
                self.notice(format!(
                    "memory file `{name}` was not saved: content or metadata too long"
                ));
                self.uploaded.insert(name, digest);
                continue;
            }
            self.uploaded.insert(name, digest);
            out.push(write);
        }
        out
    }

    pub fn push_notice(&mut self, n: String) {
        self.notice(n);
    }

    fn notice(&mut self, n: String) {
        if self.notices.len() < 20 {
            self.notices.push(n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_roundtrip_and_tamper() {
        let f = seal("# Hi\nbody");
        assert!(verify(&f));
        assert_eq!(strip_trailer(&f), "# Hi\nbody\n");
        assert!(!verify(&f.replacen("body", "bodY", 1)));
        assert!(!verify("no trailer"));
    }

    #[test]
    fn front_matter() {
        let p = parse_memory("---\ntitle: \"T\"\ndescription: D\n---\nhello\n");
        assert_eq!(p.title.as_deref(), Some("T"));
        assert_eq!(p.description.as_deref(), Some("D"));
        assert_eq!(p.content, "hello\n");
    }

    #[test]
    fn unquote_double_quoted_unescapes_like_the_renderer() {
        assert_eq!(unquote(r#" "Cafe \"menu\"" "#), r#"Cafe "menu""#);
        assert_eq!(unquote(r#""a\\b""#), r"a\b");
        assert_eq!(unquote(r#""l1\nl2\tx""#), "l1\nl2\tx");
        assert_eq!(unquote(r#""café 😀""#), "café 😀");
        assert_eq!(unquote(r#""""#), "");
    }

    #[test]
    fn unquote_single_quoted_unescapes_doubled_quote() {
        assert_eq!(unquote("'it''s'"), "it's");
        assert_eq!(unquote(r"'a\b'"), r"a\b");
        assert_eq!(unquote("''"), "");
    }

    #[test]
    fn unquote_unquoted_is_trimmed() {
        assert_eq!(unquote("  plain \\n text "), "plain \\n text");
        assert_eq!(unquote("\"half"), "\"half");
        assert_eq!(unquote("\""), "\"");
    }

    #[test]
    fn unquote_malformed_double_quoted_falls_back_to_inner_text() {
        // An invalid escape and an unescaped inner quote: not valid JSON, so the inner text stays.
        assert_eq!(unquote(r#""bad \q escape""#), r"bad \q escape");
        assert_eq!(unquote(r#""say "hi" now""#), r#"say "hi" now"#);
    }

    #[test]
    fn keys() {
        assert!(valid_key("a-b-1"));
        assert!(!valid_key("A"));
        assert!(!valid_key("a--b"));
        assert!(!valid_key("-a"));
    }
}
