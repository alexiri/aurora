//! Home projection (`docs/12-supervisor-harness.md` §Home projection / §Sync protocol).
//!
//! Parts of one generation are staged on disk under `.aurora/sync/` (never buffered in memory) and
//! idempotently overwritten on replay. Only [`HomeProjector::commit`] touches the live projection:
//! staged files are installed, managed roots are pruned to the manifest and instruction
//! files are re-rendered. A crash before commit leaves the previous projection intact; the next
//! generation (or a restart of the same one at part 0) wipes the stale staging area.
//!
//! Entry shapes (the wire leaves them untyped, so they are defined here):
//! - `skills`: `{path, content}`, `path` inside the harness skills dir (files become 0444). A
//!   file that isn't UTF-8 (FIX-292) has `content: null` and its bytes in `content_base64`; one
//!   that does not decode is skipped like a file the home cannot hold. No other part is binary.
//! - `memory`: `{path: "memory/<key>.md", content}` (supervisor appends the checksum trailer; 0644,
//!   or 0444 when the entry carries `locked: true` — P6-MEMLOCK, C-1149: a locked memory is
//!   installed read-only and the harvest restores it instead of uploading it)
//!   and `{path: "memory/index.md", content}` (verbatim, 0444).
//! - `render_inputs`: objects whose keys are [`RenderInputs`] fields; merged across entries.
//! - `skills` entries: `{path, content, exec?}`; `exec: true` installs the file 0555 instead of 0444.
//! - `tools`: opaque catalogue entries, stored as `.aurora/tools.json`.
//! - `helpers` (P6-TPL-6, C-1143): `{path, content, exec?}` at `.aurora/helpers/<path>`; the
//!   managed root is accepted only below it with the C-1141 rules, files install 0555 (exec) /
//!   0444 in 0555 directories and the root is pruned to the manifest.
//! - `manifest`: `{path}`; every managed path that should exist.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsStr;
use std::fs;
use std::io::Read as _;
use std::os::fd::AsFd as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use aurora_protocol::runner::{HomeSync, HomeSyncKind};
use base64::Engine as _;
use rustix::fs::FileType;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::harvest::{INDEX_FILE, MEMORY_DIR, read_bounded_beneath, seal, verify};
use crate::instructions::{RenderInputs, render_with};
use crate::nofollow;
use crate::pg::PgDatabase;

/// The `databases` of an installed `render_inputs.json`.
pub fn installed_databases(raw: &[u8]) -> Vec<PgDatabase> {
    serde_json::from_slice::<Value>(raw)
        .ok()
        .and_then(|v| v.get("databases").cloned())
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

const AURORA_DIR: &str = ".aurora";
/// (FIX-114) Bounds of the reads of files in the agent-writable `.aurora` directory: a staged
/// part (the wire bound is 200 KiB per part, a sync stages several) and the installed render
/// inputs.
const STAGED_READ_MAX: u64 = 8 * 1024 * 1024;
const RENDER_INPUTS_READ_MAX: u64 = 8 * 1024 * 1024;
const SYNC_DIR: &str = ".aurora/sync";

#[derive(Debug, thiserror::Error)]
pub enum HomeError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("generation {0} is incomplete: {1}")]
    Incomplete(u64, &'static str),
}

#[derive(Debug, Clone)]
pub struct HomeConfig {
    pub root: PathBuf,
    /// Harness skills dir, relative to the root.
    pub skills_dir: Option<String>,
    /// Instruction file names, relative to the root.
    pub instruction_files: Vec<String>,
    /// Loopback port of the database relay; when set, the `databases` of the render inputs become
    /// a libpq service file under `.aurora/pg` (`crate::pg`, P2-DBL-2).
    pub pg: Option<u16>,
    /// (P5-GUIDE-6, C-653) The sandbox has the `aurora-agent` wrapper: the instruction file then
    /// carries its sentences. Decided by the supervisor at start, never by the API.
    pub agent_cli: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PartOutcome {
    Staged,
    /// The final part arrived; the caller harvests, then calls [`HomeProjector::commit`].
    ReadyToCommit(u64),
}

#[derive(Deserialize)]
struct FileEntry {
    path: String,
    /// Text; null beside `content_base64`.
    #[serde(default)]
    content: Option<String>,
    /// (FIX-292) Skill files only: the bytes of a file that isn't UTF-8.
    #[serde(default)]
    content_base64: Option<String>,
    /// Skill files only: install as 0555 instead of 0444 (`14` skills, executable bit).
    #[serde(default)]
    exec: bool,
    /// (P6-MEMLOCK, C-1149) Memory files only: install as 0444 and never harvest an edit; an
    /// older API never sends it, so the default is the old behaviour.
    #[serde(default)]
    locked: bool,
}

/// An entry of a part that carries text only (apps, helpers).
fn text_entry(v: &Value) -> Option<FileEntry> {
    serde_json::from_value::<FileEntry>(v.clone())
        .ok()
        .filter(|e| e.content_base64.is_none())
}

#[derive(Deserialize)]
struct ManifestEntry {
    path: String,
}

/// SHA-256 of staged bytes.
type Digest32 = [u8; 32];

fn digest_of(bytes: &[u8]) -> Digest32 {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).into()
}

/// (SEC-171) What the supervisor remembers of a staged file entry, never its content: the bytes
/// stay in the staging area, which the agent can write.
#[derive(Debug, Clone, Copy)]
struct Staged {
    /// Of the bytes the API sent (a memory file's with its trailer); `None` for a file too
    /// large to install.
    digest: Option<Digest32>,
    /// Install as 0555. The staged file's own mode says nothing.
    exec: bool,
}

/// The staged documents of a generation, as the commit read and checked them.
struct StagedDocs {
    manifest: Vec<u8>,
    tools: Option<Vec<u8>>,
    render_inputs: Option<Vec<u8>>,
}

const MANIFEST_DOC: &str = "manifest.json";
const TOOLS_DOC: &str = "tools.json";
const RENDER_INPUTS_DOC: &str = "render_inputs.json";

/// (SEC-171) The refusal of a staged file that is not what was sent, logged with the path it
/// was staged for (never its content). The generation is not acked, so it is sent again.
fn swapped(generation: u64, path: &Path) -> HomeError {
    tracing::warn!(
        generation,
        path = %path.to_string_lossy().escape_debug(),
        "staged file is not what was sent; generation refused"
    );
    HomeError::Incomplete(generation, "a staged file is not what was sent")
}

#[derive(Debug)]
pub struct HomeProjector {
    cfg: HomeConfig,
    /// A part of the current generation was invalid or failed; the generation must not be acked.
    failed: Option<&'static str>,
    current: Option<u64>,
    /// The generation this process is staging and the lowest part index it has not been given.
    /// Set by part 0 only: what an earlier process staged is never finished by this one.
    staging: Option<(u64, u32)>,
    /// (SEC-171) Every staged file of the current generation, by destination (home-relative);
    /// reset with the staging area. Only these are installed, and only with these bytes.
    staged: BTreeMap<PathBuf, Staged>,
    /// (SEC-171) The same for the documents merged from several parts, by staged name.
    staged_docs: BTreeMap<&'static str, Digest32>,
    /// (P6-MEMLOCK, C-1149) The memory files staged as locked in the current generation (their
    /// rel paths); reset with the staging area. Only a memory file entry arrives with the flag.
    locked_keys: BTreeSet<PathBuf>,
    /// (FIX-258) The skill and helper files of the current generation that could not be
    /// staged; reset with the staging area.
    skipped: BTreeSet<PathBuf>,
    /// (P6-MEMLOCK, C-1149) The sealed bytes of every installed locked memory, key → file bytes;
    /// held in memory only. The harvest restores them instead of uploading an agent's edit.
    locked_bytes: Arc<HashMap<String, Vec<u8>>>,
    /// (FIX-258) The uid the agent runs as: the only files whose mode a lock changes.
    agent_uid: u32,
    /// (FIX-297) What this process installed, from the bytes it installed: the files are in
    /// the agent's home and are not read back.
    installed: Installed,
}

#[derive(Debug, Default)]
struct Installed {
    /// A generation was committed.
    committed: bool,
    tools: Option<Vec<u8>>,
    /// The render inputs last rendered: from a commit, or from the one read at boot.
    render_inputs: Option<Vec<u8>>,
    /// The text of every instruction file, once all were written.
    instructions: Option<String>,
    core_only: bool,
    databases: bool,
}

/// Relative, normal components only.
fn clean_rel(p: &str) -> Option<PathBuf> {
    if p.is_empty() || p.contains('\0') {
        return None;
    }
    let path = Path::new(p);
    path.components()
        .all(|c| matches!(c, Component::Normal(_)))
        .then(|| path.to_path_buf())
}

/// (C-643) Root of the app folders in the home.
pub const APPS_DIR: &str = ".aurora/apps";

/// (P6-TPL-6, C-1143) Root of the helper files in the home: a managed root, read-only to the
/// agent, pruned to the manifest at commit.
pub const HELPERS_DIR: &str = ".aurora/helpers";

/// App id grammar (`09 §3`): `[a-z][a-z0-9_-]{1,47}`.
fn app_id_ok(s: &str) -> bool {
    let b = s.as_bytes();
    (2..=48).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-' || *c == b'_')
}

/// (C-1134) The only files of an app folder: the ones Aurora writes. Package-shipped files are not
/// here any more (an app's skills go to the skills folder). `guide.md` is the pre-P5-APPSKILL name of
/// `app.md`: an API one release older still sends it, so the sync commits instead of being refused.
fn app_file_ok(s: &str) -> bool {
    matches!(s, "app.md" | "tools.md" | "api.json" | "guide.md")
}

/// Exactly `.aurora/apps/<id>/<name>`, nothing deeper, both segments in their grammars.
fn is_app_path(rel: &Path) -> bool {
    let mut c = rel.components().map(|c| match c {
        Component::Normal(s) => s.to_str(),
        _ => None,
    });
    matches!(
        (c.next(), c.next(), c.next(), c.next(), c.next()),
        (Some(Some(".aurora")), Some(Some("apps")), Some(Some(id)), Some(Some(f)), None)
            if app_id_ok(id) && app_file_ok(f)
    )
}

/// (P6-TPL-6, C-1143) Below `.aurora/helpers/` with the C-1141 helper path rules. The rules are
/// the shared `aurora_template::helper_path_ok` check (at most 4 segments, no `..`, no hidden or
/// empty segment) — the same function the API's writer calls, never a copy. A refused entry is a
/// warning in the staging arm, never a write elsewhere.
fn is_helper_path(rel: &Path) -> bool {
    let mut c = rel.components().map(|c| match c {
        Component::Normal(s) => s.to_str(),
        _ => None,
    });
    if !matches!(
        (c.next(), c.next()),
        (Some(Some(".aurora")), Some(Some("helpers")))
    ) {
        return false;
    }
    let mut segs = Vec::new();
    for s in c {
        match s {
            Some(seg) => segs.push(seg),
            None => return false,
        }
    }
    !segs.is_empty() && aurora_template::helper_path_ok(&segs.join("/"))
}

/// The errors that are about the file's own place (a file where a folder is needed or the
/// reverse, a name that is too long, an oversized staged file): trying again gives the same.
/// Any other one (a full disk) fails the generation, which is then sent again.
fn cannot_hold(error: &std::io::Error) -> bool {
    use std::io::ErrorKind as K;
    matches!(
        error.kind(),
        K::AlreadyExists
            | K::NotADirectory
            | K::IsADirectory
            | K::DirectoryNotEmpty
            | K::InvalidFilename
            | K::InvalidData
    )
}

/// A skill or helper file whose new version is skipped must be seen to be missing, never be
/// the old one beside the new rest: it leaves `keep` and the home.
fn discard(root: &Path, keep: &mut BTreeSet<PathBuf>, rel: &Path) {
    keep.remove(rel);
    if let Err(error) = nofollow::remove_file_beneath(root, rel) {
        tracing::warn!(path = %rel.display(), %error, "old version of a skipped file not removed");
    }
}

/// Where a staged file is, relative to the home.
fn staged_rel(rel: &Path) -> PathBuf {
    Path::new(SYNC_DIR).join("files").join(rel)
}

fn too_large() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, "staged file too large")
}

/// The bytes of an open staged file, if they are the ones `expected` names. `None` for any
/// others, a longer file included (no file over the bound has a digest to match).
fn read_sent(file: fs::File, expected: &Digest32) -> std::io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    file.take(STAGED_READ_MAX + 1).read_to_end(&mut bytes)?;
    Ok((digest_of(&bytes) == *expected).then_some(bytes))
}

/// Installs one staged file, `file` being `src` already opened and checked, at `dst` with
/// `mode`, and removes `src`. The content comes from the open file, never from the path: the
/// staging area is in the agent's home, so `src` may name something else by now. What is
/// written is what was read and found to be `expected` (SEC-171): the same buffer. Returns the
/// installed bytes, or `None` with nothing written when the file holds anything else.
fn install_staged(
    root: &Path,
    file: fs::File,
    src: &Path,
    dst: &Path,
    mode: u32,
    expected: &Digest32,
) -> std::io::Result<Option<Vec<u8>>> {
    let Some(bytes) = read_sent(file, expected)? else {
        return Ok(None);
    };
    let outside = || std::io::Error::from(std::io::ErrorKind::InvalidInput);
    let rel = dst.strip_prefix(root).map_err(|_| outside())?;
    let staged = src.strip_prefix(root).map_err(|_| outside())?;
    // Every managed destination: no link in the chain from the home down. The folders are
    // then made and held open from the home's descriptor, so that a link planted after the
    // sweep is not followed either.
    nofollow::write_beneath(root, rel, &bytes, mode)?;
    // The whole staging area is removed after the commit anyway.
    let _ = nofollow::remove_file_beneath(root, staged);
    Ok(Some(bytes))
}

impl HomeProjector {
    pub fn new(cfg: HomeConfig) -> Self {
        Self {
            cfg,
            failed: None,
            current: None,
            staging: None,
            staged: BTreeMap::new(),
            staged_docs: BTreeMap::new(),
            locked_keys: BTreeSet::new(),
            skipped: BTreeSet::new(),
            locked_bytes: Arc::new(HashMap::new()),
            agent_uid: rustix::process::getuid().as_raw(),
            installed: Installed::default(),
        }
    }

    /// The agent runs as another uid than the supervisor (the root supervisor mode).
    pub fn set_agent_uid(&mut self, uid: u32) {
        self.agent_uid = uid;
    }

    /// Writes one file of the staging area, from the home's descriptor.
    fn write_staged(&self, rel: &Path, body: &[u8]) -> std::io::Result<()> {
        nofollow::write_beneath(&self.cfg.root, &staged_rel(rel), body, 0o600)
    }

    fn skills_prefix(&self) -> Option<PathBuf> {
        self.cfg.skills_dir.as_deref().and_then(clean_rel)
    }

    fn is_skill_path(&self, rel: &Path) -> bool {
        self.skills_prefix().is_some_and(|s| rel.starts_with(s))
    }

    /// A managed projection path: under the skills dir, `memory/<name>.md`, an app folder file
    /// `.aurora/apps/<id>/<name>` (C-643) or a helper file `.aurora/helpers/<path>`
    /// (P6-TPL-6, C-1143).
    fn managed(&self, rel: &Path) -> bool {
        if is_app_path(rel) || is_helper_path(rel) {
            return true;
        }
        if let Some(s) = self.skills_prefix()
            && rel.starts_with(&s)
            && rel != s
        {
            return true;
        }
        let mut c = rel.components();
        matches!(
            (c.next(), c.next(), c.next()),
            (Some(Component::Normal(d)), Some(Component::Normal(f)), None)
                if d == MEMORY_DIR && Path::new(f).extension().is_some_and(|e| e == "md")
        )
    }

    /// Remembers what was just written to the staging area for `rel`.
    fn record(&mut self, rel: PathBuf, body: &[u8], exec: bool) {
        let digest = (body.len() as u64 <= STAGED_READ_MAX).then(|| digest_of(body));
        self.staged.insert(rel, Staged { digest, exec });
    }

    /// A skill or helper file that is skipped. Whatever an earlier part staged for it goes too:
    /// neither remembered nor left in the staging area, so it is not installed.
    fn skip(&mut self, rel: PathBuf) {
        if self.staged.remove(&rel).is_some() {
            let staged = staged_rel(&rel);
            if let Err(error) = nofollow::remove_file_beneath(&self.cfg.root, &staged) {
                // The commit then finds a file nobody sent and refuses the generation.
                tracing::warn!(path = %rel.display(), %error, "staged version of a skipped file not removed");
            }
        }
        self.skipped.insert(rel);
    }

    /// The staged document `name`, checked against what was last written to it. `None` when no
    /// part brought one.
    fn read_doc(&self, generation: u64, name: &'static str) -> Result<Option<Vec<u8>>, HomeError> {
        let rel = Path::new(SYNC_DIR).join(name);
        let Some(expected) = self.staged_docs.get(name) else {
            return if nofollow::exists_beneath(&self.cfg.root, &rel) {
                Err(swapped(generation, Path::new(name)))
            } else {
                Ok(None)
            };
        };
        match read_bounded_beneath(&self.cfg.root, &rel, STAGED_READ_MAX) {
            Ok(Some(bytes)) if digest_of(&bytes) == *expected => Ok(Some(bytes)),
            _ => Err(swapped(generation, Path::new(name))),
        }
    }

    fn write_doc(&mut self, name: &'static str, bytes: &[u8]) -> Result<(), HomeError> {
        let rel = Path::new(SYNC_DIR).join(name);
        nofollow::write_beneath(&self.cfg.root, &rel, bytes, 0o600)?;
        self.staged_docs.insert(name, digest_of(bytes));
        Ok(())
    }

    /// (SEC-171) The whole staging area against what was sent, before anything installed is
    /// touched: every staged entry is there with its bytes, and nothing else is.
    fn verify_staged(&self, generation: u64) -> Result<StagedDocs, HomeError> {
        let root = &self.cfg.root;
        let Ok(found) = nofollow::open_dir_beneath(root, &staged_rel(Path::new("")))
            .and_then(nofollow::list_files)
        else {
            return Err(swapped(generation, Path::new("files")));
        };
        for rel in found {
            if !self.staged.contains_key(&rel) {
                return Err(swapped(generation, &rel));
            }
        }
        for (rel, entry) in &self.staged {
            // A file too large to install is never read: the install skips or refuses it.
            let Some(expected) = &entry.digest else {
                continue;
            };
            let sent = nofollow::open_regular_beneath(root, &staged_rel(rel))
                .and_then(|file| read_sent(file, expected));
            if !matches!(sent, Ok(Some(_))) {
                return Err(swapped(generation, rel));
            }
        }
        Ok(StagedDocs {
            manifest: self
                .read_doc(generation, MANIFEST_DOC)?
                .ok_or(HomeError::Incomplete(generation, "no manifest part"))?,
            tools: self.read_doc(generation, TOOLS_DOC)?,
            render_inputs: self.read_doc(generation, RENDER_INPUTS_DOC)?,
        })
    }

    /// Applies one `home.sync` part. Part 0 resets staging; any other part is taken only when
    /// this process staged every part before it (a replay of one is fine). The locked set lives
    /// in this process, so a generation staged by an earlier one, or with a part missing, would
    /// install a locked memory writable: it is refused, not acked, and sent again from part 0.
    pub fn apply_part(&mut self, sync: &HomeSync) -> Result<PartOutcome, HomeError> {
        if sync.part_index == 0 {
            self.staging = None;
            // (FIX-297) The agent can put a link at `.aurora` at any time: it is removed, and
            // the wipe and every write go from the home's descriptor.
            nofollow::clear_link_chain(&self.cfg.root, Path::new(SYNC_DIR));
            if let Err(error) = nofollow::remove_tree_beneath(&self.cfg.root, Path::new(SYNC_DIR)) {
                // The commit finds what nobody sent and refuses the generation.
                tracing::warn!(%error, "old staging area not removed");
            }
            nofollow::ensure_dir_beneath(&self.cfg.root, &staged_rel(Path::new("")))?;
            self.failed = None;
            self.staged.clear();
            self.staged_docs.clear();
            // (P6-MEMLOCK) The locked set belongs to the generation being staged.
            self.locked_keys.clear();
            self.skipped.clear();
            self.staging = Some((sync.generation, 0));
        }
        match self.staging {
            Some((g, next)) if g == sync.generation && sync.part_index <= next => {
                self.staging = Some((g, next.max(sync.part_index.saturating_add(1))));
            }
            _ => {
                return Err(HomeError::Incomplete(
                    sync.generation,
                    "an earlier part was not staged; waiting for the generation from part 0",
                ));
            }
        }
        self.current = Some(sync.generation);
        if let Err(e) = self.stage(sync) {
            self.failed = Some("part could not be staged");
            return Err(e);
        }
        if sync.part.kind == HomeSyncKind::Final || sync.part_index + 1 >= sync.part_count {
            if let Some(why) = self.failed {
                return Err(HomeError::Incomplete(sync.generation, why));
            }
            if !self.staged_docs.contains_key(MANIFEST_DOC) {
                return Err(HomeError::Incomplete(sync.generation, "no manifest part"));
            }
            return Ok(PartOutcome::ReadyToCommit(sync.generation));
        }
        Ok(PartOutcome::Staged)
    }

    fn stage(&mut self, sync: &HomeSync) -> Result<(), HomeError> {
        let entries = &sync.part.entries;
        match &sync.part.kind {
            HomeSyncKind::Skills | HomeSyncKind::Memory => {
                let memory = sync.part.kind == HomeSyncKind::Memory;
                for v in entries {
                    let Ok(e) = serde_json::from_value::<FileEntry>(v.clone()) else {
                        tracing::warn!("malformed file entry refused");
                        continue;
                    };
                    let rel = match clean_rel(&e.path) {
                        Some(r)
                            if self.managed(&r)
                                && !is_app_path(&r)
                                && !is_helper_path(&r)
                                && memory == r.starts_with(MEMORY_DIR) =>
                        {
                            r
                        }
                        _ => {
                            tracing::warn!(path = %e.path.escape_debug(), "path outside managed roots refused");
                            continue;
                        }
                    };
                    let body = match e.content_base64 {
                        None => {
                            let text = e.content.unwrap_or_default();
                            if memory && rel.file_name().is_some_and(|n| n != INDEX_FILE) {
                                seal(&text).into_bytes()
                            } else {
                                text.into_bytes()
                            }
                        }
                        Some(_) if memory => {
                            tracing::warn!(path = %e.path.escape_debug(), "binary memory entry refused");
                            continue;
                        }
                        Some(encoded) => {
                            match base64::engine::general_purpose::STANDARD.decode(encoded) {
                                Ok(bytes) => bytes,
                                Err(error) => {
                                    tracing::warn!(
                                        path = %e.path.escape_debug(),
                                        %error,
                                        "skill file does not decode; skipped"
                                    );
                                    self.skip(rel);
                                    continue;
                                }
                            }
                        }
                    };
                    // (P6-MEMLOCK) The locked flag travels through staging as a tracked path.
                    if memory && e.locked {
                        self.locked_keys.insert(rel.clone());
                    }
                    match self.write_staged(&rel, &body) {
                        Ok(()) => {
                            self.skipped.remove(&rel);
                            self.record(rel, &body, e.exec && !memory);
                        }
                        // A skill file the home cannot hold is skipped, like a helper.
                        Err(error) if !memory && cannot_hold(&error) => {
                            tracing::warn!(
                                path = %e.path.escape_debug(),
                                %error,
                                "skill file not written; skipped"
                            );
                            self.skip(rel);
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
            }
            HomeSyncKind::Apps => {
                for v in entries {
                    let Some(e) = text_entry(v) else {
                        tracing::warn!("malformed app file entry refused");
                        continue;
                    };
                    match clean_rel(&e.path) {
                        Some(r) if is_app_path(&r) => {
                            let body = e.content.unwrap_or_default().into_bytes();
                            self.write_staged(&r, &body)?;
                            self.record(r, &body, false);
                        }
                        _ => {
                            tracing::warn!(path = %e.path.escape_debug(), "app file path refused");
                        }
                    }
                }
            }
            // (P6-TPL-6, C-1143) The helper files: accepted only below `.aurora/helpers/` with
            // the C-1141 rules; a refused entry is a warning, never a write elsewhere.
            HomeSyncKind::Helpers => {
                for v in entries {
                    let Some(e) = text_entry(v) else {
                        tracing::warn!("malformed helper file entry refused");
                        continue;
                    };
                    match clean_rel(&e.path) {
                        Some(r) if is_helper_path(&r) => {
                            // One helper the home cannot hold (a file that is also a folder)
                            // is skipped: it must not stall the whole sync.
                            let body = e.content.unwrap_or_default().into_bytes();
                            match self.write_staged(&r, &body) {
                                Ok(()) => {
                                    self.skipped.remove(&r);
                                    self.record(r, &body, e.exec);
                                }
                                Err(error) => {
                                    tracing::warn!(
                                        path = %e.path.escape_debug(),
                                        %error,
                                        "helper file not written; skipped"
                                    );
                                    self.skip(r);
                                }
                            }
                        }
                        _ => {
                            tracing::warn!(
                                path = %e.path.escape_debug(),
                                "helper file path outside the managed root refused"
                            );
                        }
                    }
                }
            }
            // The three documents below are merged over several parts. What a part is merged
            // into is checked first: it must not be the agent's.
            HomeSyncKind::RenderInputs => {
                let mut merged: Map<String, Value> =
                    match self.read_doc(sync.generation, RENDER_INPUTS_DOC)? {
                        Some(b) => serde_json::from_slice(&b)?,
                        None => Map::new(),
                    };
                for v in entries {
                    if let Value::Object(o) = v {
                        merged.extend(o.clone());
                    }
                }
                self.write_doc(RENDER_INPUTS_DOC, &serde_json::to_vec(&merged)?)?;
            }
            HomeSyncKind::Tools => {
                let mut all: Vec<Value> = match self.read_doc(sync.generation, TOOLS_DOC)? {
                    Some(b) => serde_json::from_slice(&b)?,
                    None => Vec::new(),
                };
                all.extend(entries.iter().cloned());
                self.write_doc(TOOLS_DOC, &serde_json::to_vec(&all)?)?;
            }
            HomeSyncKind::Manifest => {
                let mut all: Vec<String> = match self.read_doc(sync.generation, MANIFEST_DOC)? {
                    Some(b) => serde_json::from_slice(&b)?,
                    None => Vec::new(),
                };
                for v in entries {
                    if let Ok(m) = serde_json::from_value::<ManifestEntry>(v.clone()) {
                        all.push(m.path);
                    }
                }
                self.write_doc(MANIFEST_DOC, &serde_json::to_vec(&all)?)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Installs the staged generation: installs the staged files, prunes managed roots to the
    /// manifest (agent-edited memory files are kept until superseded), renders instructions.
    pub fn commit(&mut self, generation: u64) -> Result<(), HomeError> {
        // (SEC-171) Refused here, the installed generation is left as it is. A file swapped
        // while the commit runs is refused at its install: nothing but what was sent is ever
        // installed, and without an ack the generation is sent again.
        let docs = self.verify_staged(generation)?;
        let mut memories = Vec::new();
        if let Err(error) = self.install_verified(generation, &docs, &mut memories) {
            // Stopped part way: what is installed is guarded as it now stands.
            if let Some(rel) = self.skills_prefix() {
                lock_tree(&self.cfg.root, &rel, self.agent_uid);
            }
            for rel in [APPS_DIR, HELPERS_DIR] {
                lock_tree(&self.cfg.root, Path::new(rel), self.agent_uid);
            }
            let mut locked = (*self.locked_bytes).clone();
            for (key, bytes) in memories {
                match bytes {
                    Some(bytes) => locked.insert(key, bytes),
                    None => locked.remove(&key),
                };
            }
            self.locked_bytes = Arc::new(locked);
            return Err(error);
        }
        self.current = Some(generation);
        self.installed.committed = true;
        // (P6-MEMLOCK) What the harvest protects from now on; an empty map unlocks everything.
        let locked = memories
            .into_iter()
            .filter_map(|(key, bytes)| Some((key, bytes?)))
            .collect();
        self.locked_bytes = Arc::new(locked);
        Ok(())
    }

    /// The install of a generation [`Self::verify_staged`] passed. `memories` gets every memory
    /// file as it is installed: its key, and its bytes when it is a locked one.
    fn install_verified(
        &mut self,
        generation: u64,
        docs: &StagedDocs,
        memories: &mut Vec<(String, Option<Vec<u8>>)>,
    ) -> Result<(), HomeError> {
        let root = self.cfg.root.clone();
        let manifest: Vec<String> = serde_json::from_slice(&docs.manifest)?;
        let mut keep: BTreeSet<PathBuf> = manifest
            .iter()
            .filter_map(|p| clean_rel(p))
            .filter(|p| self.managed(p))
            .collect();

        let skills = self.skills_prefix();
        // (SEC-150) The agent's uid owns the home: a link in any component from the home down
        // to a managed root, or anywhere inside it, would let the supervisor's chmod, mkdir
        // and rename write outside the managed tree. Links are removed (never followed) first.
        // The same for the app folders and (P6-TPL-6, C-1143) the helpers root.
        for rel in skills
            .iter()
            .map(PathBuf::as_path)
            .chain([Path::new(APPS_DIR), Path::new(HELPERS_DIR)])
        {
            nofollow::unlink_tree_links(&root, rel);
            set_dirs(&root, rel, 0o755);
        }
        // A file that could not be staged goes now, whatever the manifest says.
        for rel in &self.skipped {
            discard(&root, &mut keep, rel);
        }
        // The skills and the helpers are pruned before the install, so that a name can turn
        // from a file into a folder (or back) between two generations.
        if let Some(rel) = &skills {
            nofollow::prune_beneath(&root, rel, &keep);
        }
        nofollow::prune_beneath(&root, Path::new(HELPERS_DIR), &keep);

        // Install.
        for (rel, entry) in &self.staged {
            let rel = rel.clone();
            let src = root.join(staged_rel(&rel));
            let dst = root.join(&rel);
            let locked = self.locked_keys.contains(&rel);
            let mode = if rel.starts_with(MEMORY_DIR) {
                if rel.file_name().is_some_and(|n| n == INDEX_FILE) || locked {
                    // (P6-MEMLOCK) A locked memory installs read-only, like the index.
                    0o444
                } else {
                    0o644
                }
            } else if entry.exec {
                0o555
            } else {
                0o444
            };
            // The staging area is in the agent's home too: only a regular file is installed,
            // and only with the bytes that were sent.
            let installed = match &entry.digest {
                Some(expected) => {
                    let Ok(file) = nofollow::open_regular_beneath(&root, &staged_rel(&rel)) else {
                        return Err(swapped(generation, &rel));
                    };
                    install_staged(&root, file, &src, &dst, mode, expected)
                }
                None => Err(too_large()),
            };
            let bytes = match installed {
                Ok(Some(b)) => b,
                Ok(None) => return Err(swapped(generation, &rel)),
                Err(error) => {
                    if !(is_helper_path(&rel) || self.is_skill_path(&rel) && cannot_hold(&error)) {
                        return Err(error.into());
                    }
                    tracing::warn!(path = %rel.display(), %error, "file not installed; skipped");
                    discard(&root, &mut keep, &rel);
                    continue;
                }
            };
            if rel.starts_with(MEMORY_DIR) {
                // The harvest restores a locked one's exact (sealed) bytes instead of
                // uploading an edit.
                match rel.file_name().and_then(|n| n.to_str()) {
                    Some(n) => {
                        let key = n.trim_end_matches(".md").to_owned();
                        memories.push((key, locked.then_some(bytes)));
                    }
                    None if locked => {
                        tracing::warn!(path = %rel.display(), "locked memory file name refused")
                    }
                    None => {}
                }
            }
            keep.insert(rel);
        }

        // Prune the other managed roots.
        // A link at `memory` must not let the prune remove files elsewhere.
        nofollow::clear_link_chain(&root, Path::new(MEMORY_DIR));
        if let Ok(list) = nofollow::open_dir_beneath(&root, Path::new(MEMORY_DIR))
            .and_then(|dir| nofollow::entries(dir.as_fd()))
        {
            for (name, kind) in list {
                let rel = Path::new(MEMORY_DIR).join(name);
                if !self.managed(&rel) || keep.contains(&rel) || kind != FileType::RegularFile {
                    continue;
                }
                let intact = rel.file_name().is_some_and(|n| n == INDEX_FILE)
                    || read_bounded_beneath(&root, &rel, crate::harvest::MEMORY_FILE_READ_MAX)
                        .ok()
                        .flatten()
                        .and_then(|b| String::from_utf8(b).ok())
                        .is_some_and(|t| verify(&t));
                if intact {
                    let _ = nofollow::remove_file_beneath(&root, &rel);
                }
            }
        }
        if let Some(rel) = &skills {
            lock_tree(&root, rel, self.agent_uid);
        }
        // The app folders: pruned to the manifest like skills (a revoked grant removes its folder).
        nofollow::prune_beneath(&root, Path::new(APPS_DIR), &keep);
        lock_tree(&root, Path::new(APPS_DIR), self.agent_uid);
        // (P6-TPL-6, C-1143) The helper files (pruned to the manifest before the install, so a
        // removed helper is gone): locked read-only (0555/0444 files, 0555 dirs).
        lock_tree(&root, Path::new(HELPERS_DIR), self.agent_uid);

        // .aurora cache + instruction files, from the bytes checked above.
        let aurora = Path::new(AURORA_DIR);
        if let Some(tools) = &docs.tools {
            nofollow::write_beneath(&root, &aurora.join(TOOLS_DOC), tools, 0o600)?;
            self.installed.tools = Some(tools.clone());
        }
        match &docs.render_inputs {
            Some(raw) => {
                nofollow::write_beneath(&root, &aurora.join(RENDER_INPUTS_DOC), raw, 0o600)?;
                // Never read back: the installed file is the agent's to rewrite.
                self.render(raw)?;
            }
            None => match self.installed.render_inputs.clone() {
                Some(raw) => self.render(&raw)?,
                None => self.render_installed()?,
            },
        }
        let _ = nofollow::remove_tree_beneath(&root, Path::new(SYNC_DIR));
        Ok(())
    }

    /// Whether the installed render inputs name databases (the relay runs only then).
    pub fn has_databases(&self) -> bool {
        self.installed.databases
    }

    /// (P6-MEMLOCK, C-1149) The sealed bytes of every installed locked memory, key → file bytes.
    /// The harvest restores them instead of uploading an agent's edit. In memory only: it is
    /// rebuilt by every committed generation.
    pub fn locked_memories(&self) -> Arc<HashMap<String, Vec<u8>>> {
        Arc::clone(&self.locked_bytes)
    }

    /// (P5-GUIDE-7, C-654) The bridge of a new harness session lists only the core tools: the
    /// installed render inputs say `on_demand` and the sandbox has `aurora-agent`. Anything else
    /// (no inputs, an older API, mode `all`, no command) lists everything, as before.
    pub fn bridge_core_only(&self) -> bool {
        self.installed.core_only
    }

    /// (P5-GUIDE-7, C-654) What a harness session captured at its start: the tool descriptors its
    /// bridge lists (the core ones when the bridge is core-only, else all) and the rendered
    /// instruction files. A change of this value is what releases idle sessions; an on-demand
    /// tool changing in a core-only sandbox does not move it. `None` until this process has
    /// committed a generation (no session runs before that), and when nothing is installed.
    pub fn catalogue_digest(&self) -> Option<String> {
        use sha2::{Digest, Sha256};
        if !self.installed.committed {
            return None;
        }
        let tools: Vec<Value> = self
            .installed
            .tools
            .as_deref()
            .and_then(|b| serde_json::from_slice(b).ok())
            .unwrap_or_default();
        let core_only = self.installed.core_only;
        let listed: Vec<&Value> = tools
            .iter()
            .filter(|t| !core_only || t.get("exposure").and_then(Value::as_str) == Some("core"))
            .collect();
        let mut h = Sha256::new();
        h.update(serde_json::to_vec(&listed).ok()?);
        let mut any = !listed.is_empty();
        if let Some(text) = &self.installed.instructions {
            for name in &self.cfg.instruction_files {
                if clean_rel(name).is_none() {
                    continue;
                }
                any = true;
                h.update([0u8]);
                h.update(name.as_bytes());
                h.update([0u8]);
                h.update(text.as_bytes());
            }
        }
        any.then(|| h.finalize().iter().map(|b| format!("{b:02x}")).collect())
    }

    /// The boot render, from the render inputs an earlier process left in the home: the one
    /// time they are read. A link, a FIFO or an oversized file is as if there were none.
    pub fn render_installed(&mut self) -> Result<(), HomeError> {
        let rel = Path::new(AURORA_DIR).join(RENDER_INPUTS_DOC);
        // (FIX-114) Bounded read: the agent's uid owns this directory.
        match read_bounded_beneath(&self.cfg.root, &rel, RENDER_INPUTS_READ_MAX) {
            Ok(Some(raw)) => self.render(&raw),
            _ => Ok(()),
        }
    }

    /// Writes what the render inputs `raw` describe: the instruction files and the files beside
    /// them.
    fn render(&mut self, raw: &[u8]) -> Result<(), HomeError> {
        self.installed.databases = !installed_databases(raw).is_empty();
        let inputs: RenderInputs = serde_json::from_slice(raw)?;
        self.installed.render_inputs = Some(raw.to_vec());
        self.installed.core_only = crate::instructions::PlatformToolsInfo::on_demand(
            inputs.platform_tools.as_ref(),
            self.cfg.agent_cli,
        );
        if let Some(port) = self.cfg.pg {
            // A failure here must not block the sync: the agent only loses `psql service=`.
            if let Err(e) = crate::pg::write(&self.cfg.root, port, &installed_databases(raw)) {
                tracing::warn!(error = %e, "libpq service file not written");
            }
        }
        // (P5-ICS, D-273) The calendar links the agent may read, as synthetic addresses; removed
        // when there are none. A failure here must not block the sync either.
        if let Err(e) = crate::links::write(&self.cfg.root, &crate::links::installed_links(raw)) {
            tracing::warn!(error = %e, "links.json not written");
        }
        // (P5-CALDAV, D-275) The servers of the person-pinned apps (addresses only); removed when
        // there are none.
        if let Err(e) =
            crate::servers::write(&self.cfg.root, &crate::servers::installed_servers(raw))
        {
            tracing::warn!(error = %e, "servers.json not written");
        }
        // (P5-SKILLDIR, C-1140) The same absolute folder as `SKILLS_DIR`.
        let skills_path = self
            .skills_prefix()
            .map(|p| crate::harness::skills_dir_abs(&self.cfg.root, &p.to_string_lossy()));
        let text = render_with(&inputs, self.cfg.agent_cli, skills_path.as_deref());
        for name in &self.cfg.instruction_files {
            let Some(rel) = clean_rel(name) else {
                tracing::warn!("instruction file name refused");
                continue;
            };
            nofollow::write_beneath(&self.cfg.root, &rel, text.as_bytes(), 0o444)?;
        }
        self.installed.instructions = Some(text);
        Ok(())
    }

    pub fn current_generation(&self) -> Option<u64> {
        self.current
    }
}

/// Sets `mode` on the folder `rel` of the home and on every folder below it; no link, at
/// `rel`, above it or inside it, is followed. A missing `rel` is nothing to do.
fn set_dirs(home: &Path, rel: &Path, mode: u32) {
    if let Ok(dir) = nofollow::open_dir_beneath(home, rel) {
        nofollow::set_tree_modes(dir, mode, None);
    }
}

/// Whether a file may be treated as the agent's (the lock changes its mode, `send_file` reads
/// it): it has this one name and is the agent's. A hard link may be another user's file
/// (`fs.protected_hardlinks=0`).
pub(crate) fn may_lock(links: u64, owner: u32, agent: u32) -> bool {
    links <= 1 && owner == agent
}

/// [`set_dirs`] at 0555 that locks the files too: executable ones stay 0555, everything else
/// is 0444. A file that is not [the agent's own](may_lock) keeps its mode.
fn lock_tree(home: &Path, rel: &Path, agent_uid: u32) {
    if let Ok(dir) = nofollow::open_dir_beneath(home, rel) {
        let file_mode = |meta: &fs::Metadata, name: &OsStr| {
            let mode = meta.permissions().mode();
            let wanted = if mode & 0o100 != 0 { 0o555 } else { 0o444 };
            // Nothing to say about a file that already has it (in the root mode Aurora's own
            // files are root's, installed with their final mode).
            if mode & 0o7777 == wanted {
                return None;
            }
            if !may_lock(meta.nlink(), meta.uid(), agent_uid) {
                tracing::warn!(
                    name = %name.to_string_lossy().escape_debug(),
                    "mode not changed: a hard link or another user's file"
                );
                return None;
            }
            Some(wanted)
        };
        nofollow::set_tree_modes(dir, 0o555, Some(&file_mode));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder with `home/` and, beside it, a file outside the home at mode 0600.
    fn scratch() -> (PathBuf, PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("aurora-home-{}", uuid::Uuid::now_v7()));
        let home = dir.join("home");
        fs::create_dir_all(&home).unwrap();
        let victim = dir.join("victim");
        fs::write(&victim, "outside").unwrap();
        fs::set_permissions(&victim, fs::Permissions::from_mode(0o600)).unwrap();
        (dir, home, victim)
    }

    fn me() -> u32 {
        rustix::process::getuid().as_raw()
    }

    fn mode_of(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    /// The staged path may become a link after it was opened and checked: what is installed is
    /// what the open file holds, never whatever the path names by then.
    #[test]
    fn a_staged_path_swapped_for_a_link_installs_the_checked_file() {
        let (dir, home, victim) = scratch();
        let (src, dst) = (home.join("staged"), home.join("managed/file"));
        fs::write(&src, "checked bytes").unwrap();
        let file = nofollow::open_regular(&src).unwrap();
        fs::remove_file(&src).unwrap();
        std::os::unix::fs::symlink(&victim, &src).unwrap();

        let expected = digest_of(b"checked bytes");
        let installed = install_staged(&home, file, &src, &dst, 0o444, &expected)
            .unwrap()
            .unwrap();

        assert!(fs::symlink_metadata(&dst).unwrap().file_type().is_file());
        assert_eq!(fs::read(&dst).unwrap(), b"checked bytes");
        assert_eq!(installed, b"checked bytes");
        assert_eq!(mode_of(&dst), 0o444);
        assert!(
            fs::symlink_metadata(&src).is_err(),
            "the staged path is removed"
        );
        assert_eq!(fs::read(&victim).unwrap(), b"outside");
        assert_eq!(mode_of(&victim), 0o600);
        fs::remove_dir_all(dir).unwrap();
    }

    /// A link may appear in a managed tree after the link sweep: locking the tree must not
    /// change the mode of what it points at.
    #[test]
    fn locking_a_tree_does_not_chmod_through_a_link() {
        let (dir, home, victim) = scratch();
        let tree = home.join("helpers");
        fs::create_dir_all(tree.join("sub")).unwrap();
        for (name, mode) in [("run.sh", 0o700), ("sub/note", 0o600)] {
            fs::write(tree.join(name), "x").unwrap();
            fs::set_permissions(tree.join(name), fs::Permissions::from_mode(mode)).unwrap();
        }
        std::os::unix::fs::symlink(&victim, tree.join("sub/link")).unwrap();

        lock_tree(&home, Path::new("helpers"), me());

        assert_eq!(mode_of(&victim), 0o600, "outside file chmodded");
        assert_eq!(mode_of(&tree.join("run.sh")), 0o555);
        assert_eq!(mode_of(&tree.join("sub/note")), 0o444);
        assert_eq!(mode_of(&tree.join("sub")), 0o555);
        set_dirs(&home, Path::new("helpers"), 0o755);
        fs::remove_dir_all(dir).unwrap();
    }

    /// (FIX-258) A hard link in a managed tree is another name of a file that may be anyone's
    /// (`fs.protected_hardlinks=0`): locking the tree leaves its mode alone.
    #[test]
    fn locking_a_tree_leaves_a_hard_linked_file_alone() {
        let (dir, home, victim) = scratch();
        fs::create_dir_all(home.join("helpers")).unwrap();
        fs::hard_link(&victim, home.join("helpers/link")).unwrap();

        lock_tree(&home, Path::new("helpers"), me());

        assert_eq!(mode_of(&victim), 0o600, "hard-linked file chmodded");
        assert_eq!(mode_of(&home.join("helpers")), 0o555);
        set_dirs(&home, Path::new("helpers"), 0o755);
        fs::remove_dir_all(dir).unwrap();
    }

    /// (FIX-258) Nor is the mode of a file the agent does not own changed (the test's files
    /// are not those of another uid it names as the agent).
    #[test]
    fn locking_a_tree_leaves_another_users_file_alone() {
        let (dir, home, _) = scratch();
        let file = home.join("helpers/file");
        fs::create_dir_all(home.join("helpers")).unwrap();
        fs::write(&file, "x").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();

        lock_tree(&home, Path::new("helpers"), me() + 1);
        assert_eq!(mode_of(&file), 0o600);
        set_dirs(&home, Path::new("helpers"), 0o755);

        lock_tree(&home, Path::new("helpers"), me());
        assert_eq!(mode_of(&file), 0o444);
        set_dirs(&home, Path::new("helpers"), 0o755);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_a_single_named_file_of_the_agent_is_locked() {
        assert!(may_lock(1, 1000, 1000));
        assert!(!may_lock(2, 1000, 1000), "a hard link");
        assert!(!may_lock(1, 0, 1000), "root's file");
    }

    /// `outside/tree/sub/file`, all at private modes, beside the home.
    fn outside_tree(dir: &Path) -> PathBuf {
        let outside = dir.join("outside");
        fs::create_dir_all(outside.join("tree/sub")).unwrap();
        fs::write(outside.join("tree/sub/file"), "outside").unwrap();
        for (p, mode) in [
            ("tree/sub/file", 0o600),
            ("tree/sub", 0o700),
            ("tree", 0o700),
        ] {
            fs::set_permissions(outside.join(p), fs::Permissions::from_mode(mode)).unwrap();
        }
        outside
    }

    fn assert_outside_modes(outside: &Path) {
        assert_eq!(mode_of(&outside.join("tree")), 0o700);
        assert_eq!(mode_of(&outside.join("tree/sub")), 0o700);
        assert_eq!(mode_of(&outside.join("tree/sub/file")), 0o600);
    }

    /// (FIX-258) A link planted after the link sweep, at the managed root, above it or inside
    /// the tree: setting the folder modes changes nothing outside the home.
    #[test]
    fn setting_folder_modes_follows_no_link() {
        let (dir, home, _) = scratch();
        let outside = outside_tree(&dir);
        // Above the root.
        std::os::unix::fs::symlink(&outside, home.join("above")).unwrap();
        // The root itself.
        std::os::unix::fs::symlink(outside.join("tree"), home.join("root")).unwrap();
        // Inside the tree.
        fs::create_dir_all(home.join("skills/s")).unwrap();
        std::os::unix::fs::symlink(outside.join("tree"), home.join("skills/s/link")).unwrap();
        for rel in ["above/tree", "root", "skills"] {
            set_dirs(&home, Path::new(rel), 0o755);
            assert_outside_modes(&outside);
        }
        assert_eq!(mode_of(&home.join("skills/s")), 0o755);
        fs::remove_dir_all(dir).unwrap();
    }

    /// (FIX-258) Folders the agent made unreadable are still reached and get the mode.
    #[test]
    fn setting_folder_modes_restores_an_unreadable_folder() {
        let (dir, home, _) = scratch();
        fs::create_dir_all(home.join("skills/s/sub")).unwrap();
        for p in ["skills/s/sub", "skills/s", "skills"] {
            fs::set_permissions(home.join(p), fs::Permissions::from_mode(0o000)).unwrap();
        }
        set_dirs(&home, Path::new("skills"), 0o755);
        for p in ["skills", "skills/s", "skills/s/sub"] {
            assert_eq!(mode_of(&home.join(p)), 0o755, "{p}");
        }
        fs::remove_dir_all(dir).unwrap();
    }

    /// (FIX-258) The folders of an installed file are opened from the home's descriptor: a
    /// link among them is refused, never created or renamed through.
    #[test]
    fn folders_are_not_made_through_a_link() {
        let (dir, home, _) = scratch();
        let outside = outside_tree(&dir);
        std::os::unix::fs::symlink(&outside, home.join("above")).unwrap();
        assert!(nofollow::create_dir_beneath(&home, Path::new("above/new")).is_err());
        assert!(!outside.join("new").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    /// (FIX-258) Nor does locking a tree whose parent became a link.
    #[test]
    fn locking_a_tree_follows_no_linked_parent() {
        let (dir, home, _) = scratch();
        let outside = outside_tree(&dir);
        std::os::unix::fs::symlink(&outside, home.join("above")).unwrap();
        lock_tree(&home, Path::new("above/tree"), me());
        assert_outside_modes(&outside);
        fs::remove_dir_all(dir).unwrap();
    }

    const SKILL: &str = ".claude/skills/s/SKILL.md";
    const RUN: &str = ".claude/skills/s/run.sh";
    const HELPER: &str = ".aurora/helpers/help.sh";
    const APP: &str = ".aurora/apps/demo/app.md";

    fn projector(home: &Path) -> HomeProjector {
        HomeProjector::new(HomeConfig {
            root: home.to_path_buf(),
            skills_dir: Some(".claude/skills".into()),
            instruction_files: vec!["AGENTS.md".into()],
            pg: None,
            agent_cli: false,
        })
    }

    /// A whole generation with one file of every staged kind; `tag` marks its content.
    fn sync_parts(generation: u64, tag: &str) -> Vec<HomeSync> {
        use serde_json::json;
        let kinds = [
            (
                "skills",
                json!([
                    {"path": SKILL, "content": format!("skill {tag}")},
                    {"path": RUN, "content": format!("run {tag}"), "exec": true}
                ]),
            ),
            (
                "memory",
                json!([{"path": "memory/pinned.md", "content": format!("pinned {tag}"), "locked": true}]),
            ),
            (
                "render_inputs",
                json!([{"agent": {"name": "Ada", "handle": "ada", "workspace": "Acme"}}]),
            ),
            ("render_inputs", json!([{"brief": format!("brief {tag}")}])),
            ("tools", json!([{"name": format!("tool_{tag}")}])),
            ("tools", json!([{"name": "memory_save"}])),
            (
                "apps",
                json!([{"path": APP, "content": format!("app {tag}")}]),
            ),
            (
                "helpers",
                json!([{"path": HELPER, "content": format!("helper {tag}"), "exec": true}]),
            ),
            (
                "manifest",
                json!([
                    {"path": SKILL}, {"path": RUN}, {"path": "memory/pinned.md"},
                    {"path": APP}, {"path": HELPER}
                ]),
            ),
            ("final", json!([])),
        ];
        let count = kinds.len();
        kinds
            .into_iter()
            .enumerate()
            .map(|(i, (kind, entries))| {
                serde_json::from_value(json!({
                    "generation": generation, "part_index": i, "part_count": count,
                    "part": {"kind": kind, "entries": entries}
                }))
                .unwrap()
            })
            .collect()
    }

    fn apply(p: &mut HomeProjector, parts: &[HomeSync]) {
        for part in parts {
            p.apply_part(part).unwrap();
        }
    }

    /// Generation 1 installed, then every part of generation 2 staged.
    fn staged_second_generation() -> (PathBuf, PathBuf, HomeProjector) {
        let (dir, home, _) = scratch();
        let mut p = projector(&home);
        apply(&mut p, &sync_parts(1, "one"));
        p.commit(1).unwrap();
        apply(&mut p, &sync_parts(2, "two"));
        (dir, home, p)
    }

    /// What the agent does to a file in its own home: new bytes, whatever its mode.
    fn rewrite(path: &Path, bytes: &str) {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(path, bytes).unwrap();
    }

    /// The home as generation 1 left it: nothing of generation 2 and nothing forged.
    fn assert_first_generation(home: &Path, p: &HomeProjector) {
        for (rel, text, mode) in [
            (SKILL, "skill one", 0o444),
            (RUN, "run one", 0o555),
            (APP, "app one", 0o444),
            (HELPER, "helper one", 0o555),
        ] {
            assert_eq!(fs::read_to_string(home.join(rel)).unwrap(), text, "{rel}");
            assert_eq!(mode_of(&home.join(rel)), mode, "{rel}");
        }
        let sealed = seal("pinned one").into_bytes();
        assert_eq!(fs::read(home.join("memory/pinned.md")).unwrap(), sealed);
        assert_eq!(mode_of(&home.join("memory/pinned.md")), 0o444);
        assert_eq!(p.locked_memories().get("pinned"), Some(&sealed));
        let agents = fs::read_to_string(home.join("AGENTS.md")).unwrap();
        assert!(agents.contains("brief one"), "{agents}");
        let tools = fs::read_to_string(home.join(".aurora/tools.json")).unwrap();
        assert!(tools.contains("tool_one"), "{tools}");
    }

    fn assert_refused(result: Result<(), HomeError>) {
        assert!(
            matches!(result, Err(HomeError::Incomplete(2, _))),
            "{result:?}"
        );
    }

    fn cleanup(dir: &Path, home: &Path) {
        for rel in [".claude/skills", APPS_DIR, HELPERS_DIR] {
            set_dirs(home, Path::new(rel), 0o755);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    /// (SEC-171) The staging area is in the agent's home: a staged locked memory and the staged
    /// render inputs (the instruction files) rewritten before the commit install nothing.
    #[test]
    fn rewritten_staged_files_refuse_the_generation() {
        let (dir, home, mut p) = staged_second_generation();
        let sync = home.join(SYNC_DIR);
        rewrite(&sync.join("files/memory/pinned.md"), &seal("forged"));
        rewrite(
            &sync.join("render_inputs.json"),
            r#"{"agent":{"name":"Eve","handle":"eve","workspace":"x"},"brief":"forged"}"#,
        );
        assert_refused(p.commit(2));
        assert_first_generation(&home, &p);
        cleanup(&dir, &home);
    }

    /// (SEC-171) The same untouched generation installs.
    #[test]
    fn untouched_staged_files_install() {
        let (dir, home, mut p) = staged_second_generation();
        p.commit(2).unwrap();
        for (rel, text) in [
            (SKILL, "skill two"),
            (APP, "app two"),
            (HELPER, "helper two"),
        ] {
            assert_eq!(fs::read_to_string(home.join(rel)).unwrap(), text);
        }
        let sealed = seal("pinned two").into_bytes();
        assert_eq!(fs::read(home.join("memory/pinned.md")).unwrap(), sealed);
        assert_eq!(p.locked_memories().get("pinned"), Some(&sealed));
        let agents = fs::read_to_string(home.join("AGENTS.md")).unwrap();
        assert!(agents.contains("brief two"), "{agents}");
        assert!(!home.join(SYNC_DIR).exists());
        cleanup(&dir, &home);
    }

    /// (SEC-171) Every staged entry is checked, one at a time: rewritten, removed, replaced by a
    /// link, or put there by someone else.
    #[test]
    fn any_swapped_staged_entry_refuses_the_generation() {
        let files = [
            format!("files/{SKILL}"),
            format!("files/{RUN}"),
            "files/memory/pinned.md".to_owned(),
            format!("files/{APP}"),
            format!("files/{HELPER}"),
        ];
        let docs = ["render_inputs.json", "tools.json", "manifest.json"];
        for rel in files.iter().map(String::as_str).chain(docs) {
            for how in ["rewritten", "removed", "linked"] {
                let (dir, home, mut p) = staged_second_generation();
                let path = home.join(SYNC_DIR).join(rel);
                match how {
                    // Valid for every kind of staged file, JSON lists included.
                    "rewritten" => rewrite(&path, "[]"),
                    "removed" => fs::remove_file(&path).unwrap(),
                    _ => {
                        fs::remove_file(&path).unwrap();
                        std::os::unix::fs::symlink(dir.join("victim"), &path).unwrap();
                    }
                }
                let result = p.commit(2);
                assert!(
                    matches!(result, Err(HomeError::Incomplete(2, _))),
                    "{rel} {how}: {result:?}"
                );
                assert_first_generation(&home, &p);
                assert_eq!(fs::read(dir.join("victim")).unwrap(), b"outside");
                cleanup(&dir, &home);
            }
        }
        let (dir, home, mut p) = staged_second_generation();
        let planted = home.join(SYNC_DIR).join("files/memory/planted.md");
        fs::write(planted, seal("planted")).unwrap();
        assert_refused(p.commit(2));
        assert!(!home.join("memory/planted.md").exists());
        assert_first_generation(&home, &p);
        cleanup(&dir, &home);
    }

    /// A generation of the given parts, a manifest of `paths` and the final part.
    fn generation_of(
        generation: u64,
        mut parts: Vec<(&str, serde_json::Value)>,
        paths: &[&str],
    ) -> Vec<HomeSync> {
        use serde_json::json;
        let manifest: Vec<_> = paths.iter().map(|p| json!({"path": p})).collect();
        parts.push(("manifest", json!(manifest)));
        parts.push(("final", json!([])));
        let count = parts.len();
        parts
            .into_iter()
            .enumerate()
            .map(|(i, (kind, entries))| {
                serde_json::from_value(json!({
                    "generation": generation, "part_index": i, "part_count": count,
                    "part": {"kind": kind, "entries": entries}
                }))
                .unwrap()
            })
            .collect()
    }

    /// (SEC-171) A path staged by one part and skipped by a later one is not installed with the
    /// earlier bytes, and does not stop the rest of the generation.
    #[test]
    fn a_file_skipped_after_it_was_staged_is_not_installed() {
        use serde_json::json;
        let (dir, home, _) = scratch();
        let mut p = projector(&home);
        apply(&mut p, &sync_parts(1, "one"));
        p.commit(1).unwrap();
        apply(
            &mut p,
            &generation_of(
                2,
                vec![
                    (
                        "skills",
                        json!([
                            {"path": SKILL, "content": "earlier"},
                            {"path": RUN, "content": "run two"}
                        ]),
                    ),
                    (
                        "skills",
                        json!([{"path": SKILL, "content": null, "content_base64": "not base64!"}]),
                    ),
                ],
                &[SKILL, RUN],
            ),
        );
        p.commit(2).unwrap();
        assert!(fs::symlink_metadata(home.join(SKILL)).is_err());
        assert_eq!(fs::read_to_string(home.join(RUN)).unwrap(), "run two");
        cleanup(&dir, &home);
    }

    /// (SEC-171) A commit that stops part way leaves the managed trees locked and protects the
    /// memories as it installed them: one locked by this generation is restored by the harvest,
    /// one it unlocked is the agent's again.
    #[test]
    fn a_commit_that_stops_part_way_still_guards_what_it_installed() {
        use serde_json::json;
        let (dir, home, _) = scratch();
        let mut p = projector(&home);
        apply(&mut p, &sync_parts(1, "one"));
        p.commit(1).unwrap();
        apply(
            &mut p,
            &generation_of(
                2,
                vec![
                    ("skills", json!([{"path": SKILL, "content": "skill two"}])),
                    ("apps", json!([{"path": APP, "content": "app two"}])),
                    (
                        "helpers",
                        json!([{"path": HELPER, "content": "helper two"}]),
                    ),
                    (
                        "memory",
                        json!([
                            {"path": "memory/fresh.md", "content": "fresh", "locked": true},
                            {"path": "memory/pinned.md", "content": "unlocked now"},
                            {"path": "memory/zz.md", "content": "never installed"}
                        ]),
                    ),
                ],
                &[
                    SKILL,
                    APP,
                    HELPER,
                    "memory/fresh.md",
                    "memory/pinned.md",
                    "memory/zz.md",
                ],
            ),
        );
        // The last file in install order cannot be written: a folder is in its place.
        fs::create_dir_all(home.join("memory/zz.md/in-the-way")).unwrap();
        assert!(p.commit(2).is_err());
        for rel in [".claude/skills", APPS_DIR, HELPERS_DIR] {
            assert_eq!(mode_of(&home.join(rel)), 0o555, "{rel}");
        }
        let locked = p.locked_memories();
        assert_eq!(locked.get("fresh"), Some(&seal("fresh").into_bytes()));
        assert_eq!(mode_of(&home.join("memory/fresh.md")), 0o444);
        assert_eq!(locked.get("pinned"), None, "unlocked by what was installed");
        assert_eq!(locked.len(), 1);
        cleanup(&dir, &home);
    }

    /// (SEC-171) A file that changes after the check, while the commit runs, is not written.
    #[test]
    fn an_install_writes_nothing_that_was_not_sent() {
        let (dir, home, _) = scratch();
        let (src, dst) = (home.join("staged"), home.join("managed/file"));
        fs::write(&src, "forged bytes").unwrap();
        let file = nofollow::open_regular(&src).unwrap();
        let expected = digest_of(b"sent bytes");
        let installed = install_staged(&home, file, &src, &dst, 0o444, &expected).unwrap();
        assert!(installed.is_none());
        assert!(fs::symlink_metadata(&dst).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    /// (SEC-171) The executable bit is what the API sent, not the staged file's mode.
    #[test]
    fn the_staged_mode_does_not_decide_the_executable_bit() {
        let (dir, home, mut p) = staged_second_generation();
        let files = home.join(SYNC_DIR).join("files");
        for (rel, mode) in [(SKILL, 0o700), (RUN, 0o600), (APP, 0o700)] {
            fs::set_permissions(files.join(rel), fs::Permissions::from_mode(mode)).unwrap();
        }
        p.commit(2).unwrap();
        assert_eq!(mode_of(&home.join(SKILL)), 0o444);
        assert_eq!(mode_of(&home.join(RUN)), 0o555);
        assert_eq!(mode_of(&home.join(APP)), 0o444);
        cleanup(&dir, &home);
    }

    /// (SEC-171) A merged document rewritten between two parts is not merged into: the sent
    /// content would then be the agent's, with a digest of its own.
    #[test]
    fn a_document_rewritten_between_parts_refuses_the_generation() {
        for (name, upto) in [("render_inputs.json", 3), ("tools.json", 5)] {
            let (dir, home, _) = scratch();
            let mut p = projector(&home);
            apply(&mut p, &sync_parts(1, "one"));
            p.commit(1).unwrap();
            let parts = sync_parts(2, "two");
            apply(&mut p, &parts[..upto]);
            rewrite(
                &home.join(SYNC_DIR).join(name),
                if name == "tools.json" {
                    r#"[{"name":"forged"}]"#
                } else {
                    r#"{"agent":{"name":"Eve","handle":"eve","workspace":"x"}}"#
                },
            );
            assert!(p.apply_part(&parts[upto]).is_err(), "{name}");
            let rest: Vec<_> = parts[upto + 1..].iter().map(|s| p.apply_part(s)).collect();
            assert!(
                matches!(rest.last(), Some(Err(HomeError::Incomplete(2, _)))),
                "{name}: {rest:?}"
            );
            assert_first_generation(&home, &p);
            cleanup(&dir, &home);
        }
    }

    /// (SEC-171) Which generation is being staged is the supervisor's memory: a marker file the
    /// agent rewrites cannot drop the parts already staged, and a supervisor that did not see a
    /// generation's first part (a restart) never completes it.
    #[test]
    fn a_generation_without_its_first_parts_is_never_ready() {
        for restart in [false, true] {
            let (dir, home, _) = scratch();
            let mut p = projector(&home);
            apply(&mut p, &sync_parts(1, "one"));
            p.commit(1).unwrap();
            let parts = sync_parts(2, "two");
            apply(&mut p, &parts[..2]);
            if restart {
                p = projector(&home);
            } else {
                fs::write(home.join(SYNC_DIR).join("generation"), "7").unwrap();
            }
            let results: Vec<_> = parts[2..].iter().map(|s| p.apply_part(s)).collect();
            let last = results.last().unwrap();
            if restart {
                assert!(
                    matches!(last, Err(HomeError::Incomplete(2, _))),
                    "{results:?}"
                );
            } else {
                assert!(
                    matches!(last, Ok(PartOutcome::ReadyToCommit(2))),
                    "{results:?}"
                );
                p.commit(2).unwrap();
                assert_eq!(fs::read_to_string(home.join(SKILL)).unwrap(), "skill two");
            }
            // Sent again from its first part, it installs.
            apply(&mut p, &sync_parts(2, "two"));
            p.commit(2).unwrap();
            assert_eq!(fs::read_to_string(home.join(SKILL)).unwrap(), "skill two");
            cleanup(&dir, &home);
        }
    }

    /// Every entry below `dir` with its mode, link target or bytes: what "untouched" means.
    fn snapshot(dir: &Path) -> BTreeMap<PathBuf, (u32, u32, Vec<u8>)> {
        fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, (u32, u32, Vec<u8>)>) {
            for e in fs::read_dir(dir).unwrap().map(Result::unwrap) {
                let path = e.path();
                let meta = fs::symlink_metadata(&path).unwrap();
                let body = if meta.is_file() {
                    fs::read(&path).unwrap()
                } else if meta.is_symlink() {
                    fs::read_link(&path)
                        .unwrap()
                        .into_os_string()
                        .into_encoded_bytes()
                } else {
                    Vec::new()
                };
                let rel = path.strip_prefix(base).unwrap().to_path_buf();
                out.insert(rel, (meta.mode(), meta.uid(), body));
                if meta.is_dir() {
                    walk(base, &path, out);
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(dir, dir, &mut out);
        out
    }

    /// A folder outside the home laid out like an `.aurora` of its own, with a file at every
    /// name the supervisor writes or removes there.
    fn decoy(dir: &Path) -> PathBuf {
        let outside = dir.join("decoy");
        for rel in [
            "sync/keep",
            "sync/files/keep",
            "sync/manifest.json",
            "tools.json",
            "render_inputs.json",
            "links.json",
            "servers.json",
            "pg/pgpass",
            "pg/pg_service.conf",
            "apps/demo/stale.md",
            "helpers/stale.sh",
            "harness/pi/sessions/r1/state",
            "harness/pi/config/auth.json",
        ] {
            let path = outside.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "outside").unwrap();
        }
        fs::create_dir_all(outside.join("terminal/claude")).unwrap();
        fs::set_permissions(
            outside.join("terminal/claude"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        outside
    }

    /// What the agent does with `.aurora`: it moves it away and puts a link in its place.
    fn link_aurora(home: &Path, outside: &Path) {
        let aurora = home.join(AURORA_DIR);
        if fs::symlink_metadata(&aurora).is_ok() {
            let away = format!("moved-away-{}", uuid::Uuid::now_v7());
            fs::rename(&aurora, home.join(away)).unwrap();
        }
        std::os::unix::fs::symlink(outside, &aurora).unwrap();
    }

    fn is_real_dir(path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
    }

    /// (FIX-297) The root mode: `.aurora` is a link out of the home before a generation starts.
    /// The link is removed, never followed, and the generation installs into a real `.aurora`.
    #[test]
    fn a_linked_aurora_is_replaced_and_never_followed() {
        let (dir, home, _) = scratch();
        let outside = decoy(&dir);
        let before = snapshot(&outside);
        link_aurora(&home, &outside);
        let mut p = projector(&home);
        apply(&mut p, &sync_parts(1, "one"));
        assert_eq!(snapshot(&outside), before, "after the parts");
        p.commit(1).unwrap();
        assert_eq!(snapshot(&outside), before, "after the commit");
        assert!(is_real_dir(&home.join(AURORA_DIR)));
        assert_first_generation(&home, &p);
        cleanup(&dir, &home);
    }

    /// (FIX-297) The link planted while a generation is staged (before any later part, or before
    /// the commit): nothing outside is written or removed, and the generation is refused.
    #[test]
    fn a_link_planted_mid_generation_refuses_it_and_is_never_followed() {
        for upto in [2, 3, 5, 8, 10] {
            let (dir, home, _) = scratch();
            let outside = decoy(&dir);
            let before = snapshot(&outside);
            let mut p = projector(&home);
            let parts = sync_parts(1, "one");
            apply(&mut p, &parts[..upto]);
            link_aurora(&home, &outside);
            let mut refused = false;
            for part in &parts[upto..] {
                if p.apply_part(part).is_err() {
                    refused = true;
                    break;
                }
            }
            assert_eq!(snapshot(&outside), before, "parts, planted at {upto}");
            if !refused {
                refused = p.commit(1).is_err();
            }
            assert_eq!(snapshot(&outside), before, "commit, planted at {upto}");
            assert!(refused, "planted at {upto}");
            // Sent again from its first part, it installs.
            apply(&mut p, &sync_parts(1, "one"));
            p.commit(1).unwrap();
            assert_eq!(snapshot(&outside), before, "resend, planted at {upto}");
            assert_first_generation(&home, &p);
            cleanup(&dir, &home);
        }
    }

    /// (FIX-297) The files rendered beside the instructions, written or removed through a
    /// linked `.aurora`.
    #[test]
    fn the_rendered_files_are_never_written_or_removed_through_a_link() {
        let (dir, home, _) = scratch();
        let outside = decoy(&dir);
        let before = snapshot(&outside);
        let id = uuid::Uuid::now_v7();
        let dbs = [PgDatabase {
            service: "prod".into(),
            connection_id: id,
        }];
        let links = crate::links::installed_links(
            format!(r#"{{"links":[{{"name":"a","app_id":"ics","connection_id":"{id}"}}]}}"#)
                .as_bytes(),
        );
        let servers = crate::servers::installed_servers(
            br#"{"servers":[{"name":"a","app_id":"caldav","url":"https://x.test/"}]}"#,
        );
        assert!(!links.is_empty() && !servers.is_empty());
        for round in ["written", "removed"] {
            let empty = round == "removed";
            link_aurora(&home, &outside);
            let _ = crate::pg::write(&home, 3130, if empty { &[] } else { &dbs });
            assert_eq!(snapshot(&outside), before, "pg {round}");
            let _ = fs::remove_file(home.join(AURORA_DIR));
            link_aurora(&home, &outside);
            let _ = crate::links::write(&home, if empty { &[] } else { &links });
            assert_eq!(snapshot(&outside), before, "links {round}");
            let _ = fs::remove_file(home.join(AURORA_DIR));
            link_aurora(&home, &outside);
            let _ = crate::servers::write(&home, if empty { &[] } else { &servers });
            assert_eq!(snapshot(&outside), before, "servers {round}");
            let _ = fs::remove_file(home.join(AURORA_DIR));
        }
        fs::remove_dir_all(dir).unwrap();
    }

    /// (FIX-297) The wipe goes through a tree however deep the agent made it, and through
    /// nothing it links to.
    #[test]
    fn a_deep_staging_tree_is_removed_without_following_links() {
        let (dir, home, _) = scratch();
        let outside = decoy(&dir);
        let before = snapshot(&outside);
        let mut deep = home.join(SYNC_DIR);
        for _ in 0..200 {
            deep.push("d");
        }
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("file"), "x").unwrap();
        std::os::unix::fs::symlink(&outside, deep.join("link")).unwrap();
        std::os::unix::fs::symlink(&outside, home.join(SYNC_DIR).join("link")).unwrap();
        nofollow::remove_tree_beneath(&home, Path::new(SYNC_DIR)).unwrap();
        assert!(fs::symlink_metadata(home.join(SYNC_DIR)).is_err());
        assert_eq!(snapshot(&outside), before);
        fs::remove_dir_all(dir).unwrap();
    }

    /// (FIX-297) Purging a harness session removes nothing through a linked `.aurora`.
    #[tokio::test]
    async fn a_session_purge_never_follows_a_link() {
        use crate::harness::HarnessAdapter as _;
        let (dir, home, _) = scratch();
        let outside = decoy(&dir);
        let before = snapshot(&outside);
        link_aurora(&home, &outside);
        let _ = crate::adapters::pi::PiAdapter::default()
            .purge_session(&home, "r1")
            .await;
        assert_eq!(snapshot(&outside), before);
        fs::remove_dir_all(dir).unwrap();
    }

    /// (FIX-297) Nor is a harness's configuration rendered, or a credential store removed,
    /// through one.
    #[test]
    fn a_harness_configuration_is_never_rendered_through_a_link() {
        let (dir, home, _) = scratch();
        let outside = decoy(&dir);
        let before = snapshot(&outside);
        link_aurora(&home, &outside);
        let _ = crate::adapters::render::render_dir(
            &home,
            &crate::adapters::render::config_dir(&home, "pi"),
            &[("models.json", "{}".into())],
            &["auth.json"],
        );
        assert_eq!(snapshot(&outside), before);
        let rendered = home.join(".aurora/harness/pi/config/models.json");
        assert_eq!(fs::read_to_string(rendered).unwrap(), "{}");
        assert!(is_real_dir(&home.join(AURORA_DIR)));
        fs::remove_dir_all(dir).unwrap();
    }

    /// (FIX-297) A FIFO where a boot file may be kept as the agent's own: it is left alone, and
    /// the boot does not wait on it.
    #[test]
    fn a_fifo_at_a_boot_file_does_not_block_the_boot() {
        let (dir, home, _) = scratch();
        let fifo = home.join("notes");
        make_fifo(&fifo);
        let doc: crate::boot::BootDocument = serde_json::from_value(serde_json::json!({
            "v": 1, "sandbox_id": uuid::Uuid::now_v7(), "boot_id": uuid::Uuid::now_v7(),
            "agent": {"id": uuid::Uuid::now_v7(), "handle": "ada", "name": "Ada"},
            "harness": {"id": "fake"},
            "runner_url": "ws://runner:1", "bootstrap_token": "t",
            "gateway": {"proxy": "gw:1", "tls": true, "boot_token": "t"},
            "ca_pem": "ca",
            "files": [{"path": fifo, "content_b64": "eA==", "only_if_absent_or_sentinel": true}]
        }))
        .unwrap();
        let (root, ca) = (home.clone(), dir.join("ca.pem"));
        let skipped = promptly(&fifo, move || {
            crate::boot::write_boot_files_in(&doc, &ca, &root)
        })
        .unwrap();
        assert_eq!(skipped, vec![fifo]);
        fs::remove_dir_all(dir).unwrap();
    }

    /// (FIX-297) A boot file replaces what is at its name: a FIFO is not waited on, and a hard
    /// link's other name keeps its content. A folder linked to another folder of the home (the
    /// agent's own dotfiles, say) stays a link.
    #[test]
    fn a_boot_file_replaces_what_is_there_and_writes_into_nothing() {
        let (dir, home, victim) = scratch();
        fs::hard_link(&victim, home.join("linked")).unwrap();
        make_fifo(&home.join("fifo"));
        fs::create_dir_all(home.join("dotfiles")).unwrap();
        std::os::unix::fs::symlink("dotfiles", home.join(".config")).unwrap();
        let files: Vec<_> = ["linked", "fifo", ".config/tool/rc"]
            .iter()
            .map(|name| serde_json::json!({"path": home.join(name), "content_b64": "eA=="}))
            .collect();
        let doc: crate::boot::BootDocument = serde_json::from_value(serde_json::json!({
            "v": 1, "sandbox_id": uuid::Uuid::now_v7(), "boot_id": uuid::Uuid::now_v7(),
            "agent": {"id": uuid::Uuid::now_v7(), "handle": "ada", "name": "Ada"},
            "harness": {"id": "fake"},
            "runner_url": "ws://runner:1", "bootstrap_token": "t",
            "gateway": {"proxy": "gw:1", "tls": true, "boot_token": "t"},
            "ca_pem": "ca", "files": files
        }))
        .unwrap();
        let (root, ca) = (home.clone(), dir.join("ca.pem"));
        promptly(&home.join("fifo"), move || {
            crate::boot::write_boot_files_in(&doc, &ca, &root)
        })
        .unwrap();
        assert_eq!(fs::read_to_string(&victim).unwrap(), "outside");
        assert_eq!(mode_of(&victim), 0o600);
        for name in ["linked", "fifo", "dotfiles/tool/rc"] {
            let path = home.join(name);
            assert!(fs::symlink_metadata(&path).unwrap().is_file(), "{name}");
            assert_eq!(fs::read_to_string(&path).unwrap(), "x", "{name}");
            assert_eq!(mode_of(&path), 0o644, "{name}");
        }
        assert!(
            fs::symlink_metadata(home.join(".config"))
                .unwrap()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(dir.join("ca.pem")).unwrap(), "ca");
        fs::remove_dir_all(dir).unwrap();
    }

    /// (FIX-297) What is not a regular file is not opened at all (opening a device node can
    /// act on the device): seen here by a FIFO's writer, which any open for reading lets go.
    #[test]
    fn what_is_not_a_regular_file_is_never_opened() {
        let (dir, home, _) = scratch();
        let fifo = home.join("fifo");
        make_fifo(&fifo);
        let path = fifo.clone();
        let writer = std::thread::spawn(move || fs::OpenOptions::new().write(true).open(path));
        let pause = || std::thread::sleep(std::time::Duration::from_millis(300));
        pause();
        let by_path = nofollow::open_regular(&fifo).is_err();
        pause();
        let released_by_path = writer.is_finished();
        let beneath = nofollow::open_regular_beneath(&home, Path::new("fifo")).is_err();
        pause();
        let released_beneath = writer.is_finished();
        // Let the writer go.
        let reader = fs::File::open(&fifo).unwrap();
        writer.join().unwrap().unwrap();
        drop(reader);
        assert!(by_path && beneath);
        assert!(!released_by_path, "opened by path");
        assert!(!released_beneath, "opened beneath");
        fs::remove_dir_all(dir).unwrap();
    }

    /// (FIX-297) The terminal wrappers' file is in a folder the agent can write: a link at the
    /// name of its temp file, or at its own, is not written through.
    #[test]
    fn the_harness_env_file_is_never_written_through_a_link() {
        let (dir, home, victim) = scratch();
        for name in ["harness.env.tmp", "harness.env"] {
            let _ = fs::remove_file(home.join("harness.env"));
            std::os::unix::fs::symlink(&victim, home.join(name)).unwrap();
            let _ = crate::harness_env::write(&home.join("harness.env"), "A='1'\n");
            assert_eq!(fs::read_to_string(&victim).unwrap(), "outside", "{name}");
            assert_eq!(mode_of(&victim), 0o600, "{name}");
            let _ = fs::remove_file(home.join(name));
        }
        crate::harness_env::write(&home.join("harness.env"), "A='1'\n").unwrap();
        assert_eq!(
            fs::read_to_string(home.join("harness.env")).unwrap(),
            "A='1'\n"
        );
        assert_eq!(mode_of(&home.join("harness.env")), 0o644);
        fs::remove_dir_all(dir).unwrap();
    }

    /// (FIX-297) The tools socket's mode is set on a socket the supervisor made and on nothing
    /// else: its folder may be the agent's to write, so another entry can take its name.
    #[test]
    fn a_socket_mode_is_never_set_through_a_link() {
        let (dir, home, victim) = scratch();
        let socket = home.join("tools.sock");
        let at = home.join("at");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        // A link to a file outside, a link to a socket, a plain file and a hard link to one.
        for how in ["link", "link to a socket", "file", "hard link"] {
            let _ = fs::remove_file(&at);
            match how {
                "link" => std::os::unix::fs::symlink(&victim, &at).unwrap(),
                "link to a socket" => std::os::unix::fs::symlink(&socket, &at).unwrap(),
                "file" => fs::write(&at, "x").unwrap(),
                _ => fs::hard_link(&victim, &at).unwrap(),
            }
            assert!(
                nofollow::set_socket_mode(&at, 0o666, me()).is_err(),
                "{how}"
            );
            assert_eq!(mode_of(&victim), 0o600, "{how}");
            assert_eq!(mode_of(&socket), 0o600, "{how}");
        }
        // Another user's socket (the test's is not the uid named here).
        assert!(nofollow::set_socket_mode(&socket, 0o666, me() + 1).is_err());
        assert_eq!(mode_of(&socket), 0o600);
        nofollow::set_socket_mode(&socket, 0o666, me()).unwrap();
        assert_eq!(mode_of(&socket), 0o666);
        fs::remove_dir_all(dir).unwrap();
    }

    /// (FIX-297) The terminal state folders are made, chmodded and chowned (as root) in the
    /// home only.
    #[test]
    fn the_terminal_state_folders_are_never_made_through_a_link() {
        let (dir, home, _) = scratch();
        let outside = decoy(&dir);
        let before = snapshot(&outside);
        link_aurora(&home, &outside);
        let _ = crate::harness_env::ensure_state_dirs(&home, me(), me());
        assert_eq!(snapshot(&outside), before);
        for name in crate::harness_env::TERMINAL_STATE_DIRS {
            assert!(is_real_dir(&home.join(".aurora/terminal").join(name)));
        }
        assert!(is_real_dir(&home.join(AURORA_DIR)));
        fs::remove_dir_all(dir).unwrap();
    }

    /// (FIX-297) An inbound attachment lands in the home, not wherever `.aurora` points.
    #[tokio::test]
    async fn an_inbound_file_is_never_written_through_a_link() {
        let (dir, home, _) = scratch();
        let outside = decoy(&dir);
        let before = snapshot(&outside);
        link_aurora(&home, &outside);
        let turn = uuid::Uuid::now_v7();
        let attachment = uuid::Uuid::now_v7();
        let mut inbox = crate::inbox::Inbox::new(&home);
        let sha = crate::harvest::tests_sha_hex(b"hello");
        let frame: aurora_protocol::supervisor::FileIn =
            serde_json::from_value(serde_json::json!({
                "turn_id": turn, "attachment_id": attachment, "filename": "a.txt",
                "size": 5, "sha256": sha, "index": 0, "last": true, "data_b64": "aGVsbG8="
            }))
            .unwrap();
        inbox.accept(frame).await;
        assert_eq!(snapshot(&outside), before);
        let landed = home.join(format!(".aurora/inbox/{turn}/a.txt"));
        assert_eq!(fs::read_to_string(&landed).unwrap(), "hello");
        assert!(is_real_dir(&home.join(AURORA_DIR)));
        fs::remove_dir_all(dir).unwrap();
    }

    /// Runs `f` on its own thread; a call stuck opening the FIFO at `fifo` is released and the
    /// test fails.
    fn promptly<T: Send + 'static>(fifo: &Path, f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || tx.send(f()));
        match rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(value) => {
                thread.join().unwrap().unwrap();
                value
            }
            Err(_) => {
                use std::os::unix::fs::OpenOptionsExt as _;
                let writer = fs::OpenOptions::new()
                    .write(true)
                    .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
                    .open(fifo);
                drop(writer);
                panic!("blocked on the FIFO");
            }
        }
    }

    fn make_fifo(path: &Path) {
        rustix::fs::mknodat(
            rustix::fs::CWD,
            path,
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o600),
            0,
        )
        .unwrap();
    }

    const WITH_DATABASE: &str = r#"{"agent":{"name":"Ada","handle":"ada","workspace":"x"},
        "platform_tools":{"mode":"on_demand","families":[]},
        "databases":[{"service":"prod","connection_id":"0192f3a0-aaaa-7bbb-8ccc-ddddeeeeffff"}]}"#;

    /// (FIX-297) The boot read of the installed render inputs: a FIFO there answers "no
    /// databases" at once.
    #[test]
    fn a_fifo_at_the_render_inputs_does_not_block_the_boot_read() {
        let (dir, home, _) = scratch();
        fs::create_dir_all(home.join(AURORA_DIR)).unwrap();
        let fifo = home.join(".aurora/render_inputs.json");
        make_fifo(&fifo);
        let root = home.clone();
        let (databases, core_only) = promptly(&fifo, move || {
            let mut p = projector(&root);
            let _ = p.render_installed();
            (p.has_databases(), p.bridge_core_only())
        });
        assert!(!databases && !core_only);
        fs::remove_dir_all(dir).unwrap();
    }

    /// (FIX-297) A link at the render inputs, or at `.aurora` itself, is not read.
    #[test]
    fn linked_render_inputs_are_not_read() {
        for whole_dir in [false, true] {
            let (dir, home, _) = scratch();
            let outside = dir.join("outside");
            fs::create_dir_all(&outside).unwrap();
            fs::write(outside.join(RENDER_INPUTS_DOC), WITH_DATABASE).unwrap();
            if whole_dir {
                link_aurora(&home, &outside);
            } else {
                fs::create_dir_all(home.join(AURORA_DIR)).unwrap();
                std::os::unix::fs::symlink(
                    outside.join(RENDER_INPUTS_DOC),
                    home.join(".aurora/render_inputs.json"),
                )
                .unwrap();
            }
            let mut p = projector(&home);
            let _ = p.render_installed();
            assert!(!p.has_databases(), "whole_dir {whole_dir}");
            assert!(!p.bridge_core_only(), "whole_dir {whole_dir}");
            assert!(!home.join("AGENTS.md").exists(), "whole_dir {whole_dir}");
            fs::remove_dir_all(dir).unwrap();
        }
    }

    /// (FIX-297) What the supervisor decides from the installed documents comes from the bytes
    /// it installed: rewriting the files, or putting a FIFO there, changes nothing.
    #[test]
    fn installed_documents_are_not_read_back() {
        let (dir, home, _) = scratch();
        let mut p = projector(&home);
        apply(&mut p, &sync_parts(1, "one"));
        p.commit(1).unwrap();
        let digest = p.catalogue_digest();
        assert!(digest.is_some() && !p.has_databases() && !p.bridge_core_only());
        for rel in [".aurora/tools.json", "AGENTS.md"] {
            rewrite(&home.join(rel), r#"[{"name":"forged"}]"#);
        }
        rewrite(&home.join(".aurora/render_inputs.json"), WITH_DATABASE);
        assert_eq!(p.catalogue_digest(), digest);
        assert!(!p.has_databases() && !p.bridge_core_only());
        // A generation without render inputs renders what was last installed, not the file.
        apply(&mut p, &generation_of(2, vec![], &[]));
        p.commit(2).unwrap();
        assert!(!p.has_databases() && !p.bridge_core_only());
        let agents = fs::read_to_string(home.join("AGENTS.md")).unwrap();
        assert!(agents.contains("brief one"), "{agents}");
        let fifo = home.join(".aurora/render_inputs.json");
        fs::remove_file(&fifo).unwrap();
        make_fifo(&fifo);
        let (after, databases, _p) =
            promptly(&fifo, move || (p.catalogue_digest(), p.has_databases(), p));
        assert_eq!(after, digest);
        assert!(!databases);
        cleanup(&dir, &home);
    }

    /// (P6-MEMLOCK, C-1149) `locked` is optional on a file entry: an older API never sends it and
    /// an unlocked memory's entry has no key at all; only `true` marks the file.
    #[test]
    fn file_entry_parses_with_and_without_locked() {
        let old: FileEntry = serde_json::from_value(serde_json::json!({
            "path": "memory/note.md", "content": "x"
        }))
        .unwrap();
        assert!(!old.locked);
        assert!(!old.exec);
        let locked: FileEntry = serde_json::from_value(serde_json::json!({
            "path": "memory/pinned.md", "content": "x", "locked": true
        }))
        .unwrap();
        assert!(locked.locked);
    }
}
