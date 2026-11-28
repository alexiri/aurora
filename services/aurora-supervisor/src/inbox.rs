//! Inbound attachments (decision S-11, `docs/10-agents-conversations.md` §Attachments).
//!
//! The runner streams each attachment as `file.in` frames ahead of the `turn.deliver` that lists it.
//! Chunks are appended to a temporary file under `<workspace>/.aurora/inbox/<turn_id>/`; when the
//! `last` chunk arrives the size and sha256 are verified and the file is renamed to a sanitized,
//! de-duplicated name. Filenames are untrusted: only `[A-Za-z0-9._-]` survives, no path
//! components, at most [`MAX_NAME`] characters. A file that is missing or corrupt when the turn
//! starts never blocks the turn; the model is told in a notice instead.

use std::collections::{HashMap, HashSet, VecDeque};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

use aurora_core::limits::{ATTACH_IN_COUNT, ATTACH_IN_FILE_MAX};
use aurora_protocol::common::Validate as _;
use aurora_protocol::runner::AttachmentRef;
use aurora_protocol::supervisor::FileIn;
use base64::Engine as _;
use rustix::fs::{AtFlags, Mode, OFlags};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt as _;
use uuid::Uuid;

use crate::nofollow;

/// Longest sanitized filename.
pub const MAX_NAME: usize = 128;
/// Total bytes one turn may bring in (declared sizes are reserved up front).
pub const TURN_QUOTA: u64 = ATTACH_IN_COUNT as u64 * ATTACH_IN_FILE_MAX;
/// Turns tracked at once; the oldest is dropped (partial files removed) beyond this.
const MAX_TRACKED_TURNS: usize = 16;
const FALLBACK_NAME: &str = "attachment";

/// `[A-Za-z0-9._-]` only, no path components, no leading dot, at most [`MAX_NAME`] characters.
/// Never empty.
pub fn sanitize_filename(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim_start_matches('.');
    if cleaned.is_empty() {
        return FALLBACK_NAME.to_owned();
    }
    // Cut the stem, not the extension, when the name is too long.
    if cleaned.len() <= MAX_NAME {
        return cleaned.to_owned();
    }
    match cleaned.rfind('.') {
        Some(dot) if cleaned.len() - dot <= 16 => {
            let ext = &cleaned[dot..];
            format!("{}{ext}", &cleaned[..MAX_NAME - ext.len()])
        }
        _ => cleaned[..MAX_NAME].to_owned(),
    }
}

/// `name`, or `stem-1.ext`, `stem-2.ext`, … until `taken` rejects none.
fn dedupe(name: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_owned();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    };
    for n in 1u32.. {
        let suffix = format!("-{n}{ext}");
        let keep = MAX_NAME.saturating_sub(suffix.len());
        let candidate = format!("{}{suffix}", &stem[..stem.len().min(keep)]);
        if !taken(&candidate) {
            return candidate;
        }
    }
    format!("{FALLBACK_NAME}-{}", Uuid::now_v7().simple())
}

/// A partial file, by the folder it was made in (held open) and its name there.
struct Part {
    dir: OwnedFd,
    name: String,
}

impl Part {
    fn remove(&self) {
        let _ = rustix::fs::unlinkat(&self.dir, self.name.as_str(), AtFlags::empty());
    }
}

struct Receiving {
    file: tokio::fs::File,
    tmp: Part,
    filename: String,
    size: u64,
    sha256: String,
    next: u32,
    written: u64,
    hasher: Sha256,
}

enum Entry {
    Receiving(Box<Receiving>),
    Ready(PathBuf),
    Failed(&'static str),
}

#[derive(Default)]
struct TurnInbox {
    files: HashMap<Uuid, Entry>,
    /// Declared bytes reserved so far (in-flight plus completed).
    reserved: u64,
    /// Names already handed out in this turn's directory.
    names: HashSet<String>,
}

/// Outcome of matching a turn's attachment list against what arrived.
#[derive(Debug, Default)]
pub struct Resolved {
    /// Absolute paths of the files that arrived intact, in list order.
    pub paths: Vec<PathBuf>,
    /// One line per missing or corrupt file, for the turn input.
    pub notices: Vec<String>,
}

/// Where attachments land, relative to the workspace.
const INBOX_DIR: &str = ".aurora/inbox";

pub struct Inbox {
    /// The workspace: the agent's, so every folder below it is reached from its descriptor.
    root: PathBuf,
    turns: HashMap<Uuid, TurnInbox>,
    order: VecDeque<Uuid>,
}

impl Inbox {
    /// `workspace` is the working directory (`/workspace`).
    pub fn new(workspace: &Path) -> Self {
        Self {
            root: workspace.to_path_buf(),
            turns: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// Applies one `file.in` frame. Failures are remembered per attachment and reported by
    /// [`Inbox::resolve`]; later frames of a failed attachment are ignored.
    pub async fn accept(&mut self, f: FileIn) {
        if let Err(e) = f.validate() {
            tracing::warn!(turn_id = %f.turn_id, attachment_id = %f.attachment_id, error = %e, "invalid file.in dropped");
            return;
        }
        if !self.turns.contains_key(&f.turn_id) {
            if self.order.len() >= MAX_TRACKED_TURNS
                && let Some(old) = self.order.pop_front()
            {
                self.drop_turn(old);
            }
            self.turns.insert(f.turn_id, TurnInbox::default());
            self.order.push_back(f.turn_id);
        }
        let rel = Path::new(INBOX_DIR).join(f.turn_id.to_string());
        let dir = self.root.join(&rel);
        let root = self.root.clone();
        let Some(turn) = self.turns.get_mut(&f.turn_id) else {
            return;
        };
        let outcome = match turn.files.remove(&f.attachment_id) {
            Some(Entry::Receiving(r)) => append(*r, &f).await,
            Some(done @ (Entry::Ready(_) | Entry::Failed(_))) => Step::Settled(done),
            None => {
                if turn.files.len() >= ATTACH_IN_COUNT as usize {
                    tracing::warn!(turn_id = %f.turn_id, "too many inbound attachments; frame dropped");
                    return;
                }
                match begin(turn, &root, &rel, &f) {
                    Ok(r) => append(r, &f).await,
                    Err(why) => Step::Settled(Entry::Failed(why)),
                }
            }
        };
        match outcome {
            Step::More(r) => {
                turn.files.insert(f.attachment_id, Entry::Receiving(r));
            }
            Step::Finish(r) => {
                let entry = finish(turn, &dir, *r).await;
                if let Entry::Failed(why) = &entry {
                    tracing::warn!(turn_id = %f.turn_id, attachment_id = %f.attachment_id, reason = why, "inbound attachment discarded");
                }
                turn.files.insert(f.attachment_id, entry);
            }
            Step::Settled(entry) => {
                turn.files.insert(f.attachment_id, entry);
            }
        }
    }

    /// Consumes the turn's inbox state: paths of intact files plus a notice for each listed
    /// attachment that is missing or corrupt. Unfinished partial files are removed.
    pub async fn resolve(&mut self, turn_id: Uuid, list: &[AttachmentRef]) -> Resolved {
        let mut turn = self.turns.remove(&turn_id).unwrap_or_default();
        self.order.retain(|t| *t != turn_id);
        let mut out = Resolved::default();
        for a in list {
            let why = match turn.files.remove(&a.attachment_id) {
                Some(Entry::Ready(p)) => {
                    out.paths.push(p);
                    continue;
                }
                Some(Entry::Failed(why)) => why,
                Some(Entry::Receiving(r)) => {
                    r.tmp.remove();
                    "the transfer was incomplete"
                }
                None => "it was not received",
            };
            out.notices.push(format!(
                "Attachment `{}` ({} bytes) could not be delivered: {why}. It is not available in the workspace.",
                sanitize_filename(&a.filename),
                a.size
            ));
        }
        for (_, e) in turn.files {
            if let Entry::Receiving(r) = e {
                r.tmp.remove();
            }
        }
        out
    }

    fn drop_turn(&mut self, turn_id: Uuid) {
        if let Some(t) = self.turns.remove(&turn_id) {
            for (_, e) in t.files {
                if let Entry::Receiving(r) = e {
                    r.tmp.remove();
                }
            }
        }
    }
}

enum Step {
    More(Box<Receiving>),
    Finish(Box<Receiving>),
    Settled(Entry),
}

/// A new file `name` in the folder `rel` of the workspace: no link is followed, and what is
/// already at `name` is removed, never opened.
fn create_part(root: &Path, rel: &Path, name: String) -> std::io::Result<(Part, tokio::fs::File)> {
    let dir = nofollow::ensure_dir_beneath(root, rel)?;
    let _ = rustix::fs::unlinkat(&dir, name.as_str(), AtFlags::empty());
    let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let file = rustix::fs::openat(&dir, name.as_str(), flags, Mode::from_raw_mode(0o666))?;
    let file = tokio::fs::File::from_std(std::fs::File::from(file));
    Ok((Part { dir, name }, file))
}

/// First chunk: checks the quota and opens the temporary file.
fn begin(
    turn: &mut TurnInbox,
    root: &Path,
    rel: &Path,
    f: &FileIn,
) -> Result<Receiving, &'static str> {
    if f.index != 0 {
        return Err("its first chunk was lost");
    }
    if f.size > ATTACH_IN_FILE_MAX || turn.reserved.saturating_add(f.size) > TURN_QUOTA {
        return Err("it exceeds the inbound size limit");
    }
    let (tmp, file) = create_part(root, rel, format!(".part-{}", f.attachment_id))
        .map_err(|_| "the workspace is not writable")?;
    turn.reserved += f.size;
    Ok(Receiving {
        file,
        tmp,
        filename: f.filename.clone(),
        size: f.size,
        sha256: f.sha256.to_ascii_lowercase(),
        next: 0,
        written: 0,
        hasher: Sha256::new(),
    })
}

/// Removes the partial file and records why the attachment is lost.
async fn discard(r: Receiving, why: &'static str) -> Step {
    drop(r.file);
    r.tmp.remove();
    Step::Settled(Entry::Failed(why))
}

/// Appends one chunk; any inconsistency discards the file.
async fn append(mut r: Receiving, f: &FileIn) -> Step {
    if f.index != r.next {
        return discard(r, "a chunk was lost or out of order").await;
    }
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&f.data_b64) else {
        return discard(r, "a chunk was corrupt").await;
    };
    if r.written + bytes.len() as u64 > r.size {
        return discard(r, "it was larger than announced").await;
    }
    if r.file.write_all(&bytes).await.is_err() {
        return discard(r, "the workspace is not writable").await;
    }
    r.hasher.update(&bytes);
    r.written += bytes.len() as u64;
    r.next += 1;
    if f.last {
        Step::Finish(Box::new(r))
    } else {
        Step::More(Box::new(r))
    }
}

/// Last chunk: verify size and sha256, then atomically move to the final name.
async fn finish(turn: &mut TurnInbox, dir: &Path, mut r: Receiving) -> Entry {
    let lost = |tmp: Part, why| {
        tmp.remove();
        Entry::Failed(why)
    };
    if r.file.flush().await.is_err() || r.file.sync_all().await.is_err() {
        return lost(r.tmp, "the workspace is not writable");
    }
    drop(r.file);
    if r.written != r.size {
        return lost(r.tmp, "its size did not match");
    }
    if hex::encode(r.hasher.finalize()) != r.sha256 {
        return lost(r.tmp, "its checksum did not match");
    }
    let base = sanitize_filename(&r.filename);
    let mut name = dedupe(&base, |n| turn.names.contains(n));
    // A name may also exist from an earlier delivery of the same turn.
    while rustix::fs::statat(&r.tmp.dir, name.as_str(), AtFlags::SYMLINK_NOFOLLOW).is_ok() {
        turn.names.insert(name.clone());
        name = dedupe(&base, |n| turn.names.contains(n));
    }
    let dest = dir.join(&name);
    let part = r.tmp.name.as_str();
    if rustix::fs::renameat(&r.tmp.dir, part, &r.tmp.dir, name.as_str()).is_err() {
        return lost(r.tmp, "the workspace is not writable");
    }
    turn.names.insert(name);
    Entry::Ready(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_untrusted_names() {
        assert_eq!(sanitize_filename("../x"), "x");
        assert_eq!(sanitize_filename("/etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("a/b"), "b");
        assert_eq!(sanitize_filename("a\\b.txt"), "b.txt");
        assert_eq!(sanitize_filename(".."), FALLBACK_NAME);
        assert_eq!(sanitize_filename("."), FALLBACK_NAME);
        assert_eq!(sanitize_filename("dir/"), FALLBACK_NAME);
        assert_eq!(sanitize_filename(""), FALLBACK_NAME);
        assert_eq!(sanitize_filename(".bashrc"), "bashrc");
        assert_eq!(sanitize_filename("my report (1).pdf"), "my_report__1_.pdf");
        assert_eq!(sanitize_filename("résumé.txt"), "r_sum_.txt");
        assert_eq!(sanitize_filename("a\0b\n"), "a_b_");
        let long = format!("{}.pdf", "a".repeat(300));
        let s = sanitize_filename(&long);
        assert_eq!(s.len(), MAX_NAME);
        assert!(s.ends_with(".pdf"));
        assert_eq!(sanitize_filename(&"b".repeat(300)).len(), MAX_NAME);
    }

    #[test]
    fn dedupes_before_the_extension() {
        let taken: HashSet<&str> = ["a.txt", "a-1.txt", "noext"].into();
        let t = |n: &str| taken.contains(n);
        assert_eq!(dedupe("a.txt", t), "a-2.txt");
        assert_eq!(dedupe("noext", t), "noext-1");
        assert_eq!(dedupe("fresh", t), "fresh");
        let max = "m".repeat(MAX_NAME);
        let both: HashSet<String> = [max.clone()].into();
        assert!(dedupe(&max, |n| both.contains(n)).len() <= MAX_NAME);
    }
}
