//! Memory projection checksums and harvest of agent edits
//! (`docs/10-agents-conversations.md` §Memory, `docs/12-supervisor-harness.md` §Memory harvest).
//!
//! Every projected `memory/<key>.md` ends with `<!-- aurora:checksum sha256=<hex> -->`, the hash of
//! the whole file with the hex zeroed. A file that fails verification (or that is new) is an agent
//! edit: it is uploaded with `memory.write` and left in place until the canonical render replaces it.
//! (P6-MEMLOCK, C-1149) A locked memory is the exception: its recorded install is the truth, so an
//! edit is never uploaded — the file is restored (0444) and the agent is told the change was undone.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use aurora_core::limits::MEMORY_CONTENT_MAX;

/// (FIX-114) Largest memory file the harvest reads (four times the content limit, in bytes —
/// generous for multi-byte text); a bigger file is skipped with a notice.
pub const MEMORY_FILE_READ_MAX: u64 = 4 * MEMORY_CONTENT_MAX as u64;

/// Read at most `max` bytes of `path`: `Ok(None)` when the file is larger (never read whole).
pub fn read_bounded(path: &std::path::Path, max: u64) -> std::io::Result<Option<Vec<u8>>> {
    use std::io::Read as _;
    let mut buf = Vec::new();
    std::fs::File::open(path)?
        .take(max + 1)
        .read_to_end(&mut buf)?;
    Ok((buf.len() as u64 <= max).then_some(buf))
}
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
/// are not re-sent every tick. (P6-MEMLOCK, C-1149) `locked` is the projector's map of installed
/// locked memories (key → sealed file bytes): a locked memory is never uploaded — any change,
/// including a deleted or chmod-ed file, is undone from the recorded bytes and the agent is told
/// once per changed digest.
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

    pub fn scan(&mut self, locked: &HashMap<String, Vec<u8>>) -> Vec<MemoryWrite> {
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
        for name in &names {
            let key = name.trim_end_matches(".md");
            if let Some(recorded) = locked.get(key) {
                // (P6-MEMLOCK, C-1149) Never uploaded: the recorded install is the truth. A file
                // that is gone or oversized counts as changed and is restored from the bytes.
                let current = read_bounded(&self.dir.join(name), MEMORY_FILE_READ_MAX)
                    .ok()
                    .flatten();
                self.guard_locked(key, name, recorded, current.as_deref());
                continue;
            }
            // (FIX-114) The agent writes these files: never read one whole before checking its
            // size (a multi-gigabyte file would kill the supervisor on the memory limit).
            let text = match read_bounded(&self.dir.join(name), MEMORY_FILE_READ_MAX) {
                Ok(Some(b)) => match String::from_utf8(b) {
                    Ok(t) => t,
                    Err(_) => continue,
                },
                Ok(None) => {
                    let digest = format!("oversized:{name}");
                    if self.uploaded.get(name) != Some(&digest) {
                        self.notice(format!(
                            "memory file `{name}` was not saved: it is larger than {} KiB",
                            MEMORY_FILE_READ_MAX / 1024
                        ));
                        self.uploaded.insert(name.clone(), digest);
                    }
                    continue;
                }
                Err(_) => continue,
            };
            if verify(&text) {
                self.uploaded.remove(name);
                continue;
            }
            let digest = sha_hex(text.as_bytes());
            if self.uploaded.get(name) == Some(&digest) {
                continue;
            }
            if !valid_key(key) {
                self.notice(format!(
                    "memory file `{name}` was not saved: keys are lowercase words joined by hyphens"
                ));
                self.uploaded.insert(name.clone(), digest);
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
                self.uploaded.insert(name.clone(), digest);
                continue;
            }
            self.uploaded.insert(name.clone(), digest);
            out.push(write);
        }
        // A locked memory whose file the agent deleted is restored too.
        let mut missing: Vec<(&String, &Vec<u8>)> = locked
            .iter()
            .filter(|(k, _)| {
                !names
                    .iter()
                    .any(|n| n.trim_end_matches(".md") == k.as_str())
            })
            .collect();
        missing.sort();
        for (k, recorded) in missing {
            self.guard_locked(k, &format!("{k}.md"), recorded, None);
        }
        out
    }

    /// One locked memory file: the unmodified projection keeps only its read-only mode enforced;
    /// anything else (an edit, a deleted file) is restored from the recorded bytes with a notice
    /// per changed digest.
    fn guard_locked(&mut self, key: &str, name: &str, recorded: &[u8], current: Option<&[u8]>) {
        let path = self.dir.join(name);
        match current {
            // The unmodified projection (the recorded bytes verify by construction).
            Some(b) if b == recorded => {
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444));
                self.uploaded.remove(name);
            }
            current => {
                let digest = current.map_or_else(|| format!("missing:{name}"), sha_hex);
                if self.uploaded.get(name) != Some(&digest) {
                    self.notice(format!(
                        "memory `{key}` is locked: a person set it, and you can read it but not \
                         change or delete it. Your change was undone."
                    ));
                    self.uploaded.insert(name.to_owned(), digest);
                }
                self.restore_locked(name, recorded);
            }
        }
    }

    /// Rewrites a locked memory file from its recorded (sealed) bytes, 0444.
    fn restore_locked(&self, name: &str, recorded: &[u8]) {
        use std::io::Write as _;
        let path = self.dir.join(name);
        let tmp = self.dir.join(format!(".{name}.aurora-tmp"));
        let done = std::fs::File::create(&tmp)
            .and_then(|mut f| {
                f.write_all(recorded)?;
                f.set_permissions(std::fs::Permissions::from_mode(0o444))
            })
            .and_then(|()| std::fs::rename(&tmp, &path));
        if let Err(e) = done {
            tracing::warn!(name = %name, error = %e, "locked memory file not restored");
            let _ = std::fs::remove_file(&tmp);
        }
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
    use uuid::Uuid;

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

    fn write_keys(writes: &[MemoryWrite]) -> Vec<&str> {
        writes.iter().map(|w| w.key.as_str()).collect()
    }

    /// (P6-MEMLOCK, C-1149) `scan` with locked keys: an edited, deleted or chmod-ed locked file is
    /// restored from the recorded bytes (0444) with one notice per changed digest and never
    /// uploaded; the unlocked file beside it is harvested as usual; with no locked keys at all
    /// (unlock) the file is harvestable again.
    #[test]
    fn scan_never_uploads_a_locked_key_and_restores_it() {
        let dir = std::env::temp_dir().join(format!("aurora-harvest-{}", Uuid::now_v7()));
        std::fs::create_dir_all(dir.join(MEMORY_DIR)).unwrap();
        let sealed = seal("the person's text\n");
        std::fs::write(dir.join(MEMORY_DIR).join("pinned.md"), &sealed).unwrap();
        std::fs::write(dir.join(MEMORY_DIR).join("free.md"), seal("agent text\n")).unwrap();
        let mut locked = HashMap::new();
        locked.insert("pinned".to_owned(), sealed.clone().into_bytes());
        let mode_of = |name: &str| {
            std::os::unix::fs::PermissionsExt::mode(
                &std::fs::metadata(dir.join(MEMORY_DIR).join(name))
                    .unwrap()
                    .permissions(),
            ) & 0o777
        };
        let mut h = Harvester::new(&dir);
        let edit = |name: &str, text: &str| {
            let p = dir.join(MEMORY_DIR).join(name);
            let mut perm = std::fs::metadata(&p).unwrap().permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o644);
            std::fs::set_permissions(&p, perm).unwrap();
            std::fs::write(&p, text).unwrap();
        };

        // Clean: nothing uploaded, no notice.
        assert!(h.scan(&locked).is_empty());
        assert!(h.take_notices().is_empty());

        // An edit is restored and noticed once per digest.
        edit("pinned.md", "agent rewrote it\n");
        assert!(
            h.scan(&locked).is_empty(),
            "a locked memory is never uploaded"
        );
        assert_eq!(
            std::fs::read(dir.join(MEMORY_DIR).join("pinned.md")).unwrap(),
            sealed.as_bytes()
        );
        assert_eq!(mode_of("pinned.md"), 0o444);
        let notices = h.take_notices();
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0].contains("memory `pinned` is locked"),
            "{notices:?}"
        );
        // The same edit again: the notice is deduped by digest.
        edit("pinned.md", "agent rewrote it\n");
        let _ = h.scan(&locked);
        assert!(h.take_notices().is_empty());

        // A deleted locked file is restored with its own notice.
        std::fs::remove_file(dir.join(MEMORY_DIR).join("pinned.md")).unwrap();
        assert!(h.scan(&locked).is_empty());
        assert_eq!(
            std::fs::read(dir.join(MEMORY_DIR).join("pinned.md")).unwrap(),
            sealed.as_bytes()
        );
        assert_eq!(h.take_notices().len(), 1);

        // A chmod-ed locked file is set back to 0444 without a notice.
        edit("pinned.md", &sealed);
        assert!(h.scan(&locked).is_empty());
        assert_eq!(mode_of("pinned.md"), 0o444);
        assert!(h.take_notices().is_empty());

        // The unlocked file beside it is harvested as usual.
        edit("free.md", "agent edited this\n");
        let writes = h.scan(&locked);
        assert_eq!(write_keys(&writes), ["free"], "{writes:?}");

        // After the unlock (an empty map) an edit of the file is uploaded again.
        edit("pinned.md", "the agent tries again\n");
        let writes = h.scan(&HashMap::new());
        assert_eq!(write_keys(&writes), ["pinned"], "{writes:?}");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
