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

use std::collections::{BTreeSet, HashMap};
use std::ffi::OsStr;
use std::fs;
use std::io::Read as _;
use std::os::fd::AsFd as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use aurora_protocol::runner::{HomeSync, HomeSyncKind};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::harvest::{INDEX_FILE, MEMORY_DIR, seal, verify};
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

#[derive(Debug)]
pub struct HomeProjector {
    cfg: HomeConfig,
    /// A part of the current generation was invalid or failed; the generation must not be acked.
    failed: Option<&'static str>,
    current: Option<u64>,
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

fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    nofollow::write_atomic(path, bytes, mode)
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

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.filter_map(Result::ok) {
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() {
            walk_files(&e.path(), out);
        } else if t.is_file() || t.is_symlink() {
            out.push(e.path());
        }
    }
}

/// Removes every file under `dir` that `keep` (paths relative to `root`) does not name, then
/// every folder that is left empty.
fn prune_to(root: &Path, dir: &Path, keep: &BTreeSet<PathBuf>) {
    let mut files = Vec::new();
    walk_files(dir, &mut files);
    for f in files {
        if f.strip_prefix(root).is_ok_and(|r| !keep.contains(r)) {
            let _ = fs::remove_file(&f);
        }
    }
    let mut dirs = Vec::new();
    walk_dirs(dir, &mut dirs);
    for d in dirs {
        let _ = fs::remove_dir(&d); // only succeeds when empty
    }
}

/// Installs one staged file, `file` being `src` already opened and checked, at `dst` with
/// `mode`, and removes `src`. The content comes from the open file, never from the path: the
/// staging area is in the agent's home, so `src` may name something else by now. Returns the
/// installed bytes.
fn install_staged(
    root: &Path,
    file: fs::File,
    src: &Path,
    dst: &Path,
    mode: u32,
) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(STAGED_READ_MAX + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > STAGED_READ_MAX {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "staged file too large",
        ));
    }
    let outside = || std::io::Error::from(std::io::ErrorKind::InvalidInput);
    let rel = dst.strip_prefix(root).map_err(|_| outside())?;
    let (parent, name) = rel.parent().zip(rel.file_name()).ok_or_else(outside)?;
    // Every managed destination: no link in the chain from the home down. The folders are
    // then made and held open from the home's descriptor, so that a link planted after the
    // sweep is not followed either.
    clear_link_chain(root, parent);
    let dir = nofollow::create_dir_beneath(root, parent)?;
    nofollow::write_atomic_at(dir.as_fd(), name, &bytes, mode)?;
    // The whole staging area is removed after the commit anyway.
    let _ = fs::remove_file(src);
    Ok(bytes)
}

fn walk_dirs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.filter_map(Result::ok) {
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            walk_dirs(&e.path(), out);
            out.push(e.path());
        }
    }
}

impl HomeProjector {
    pub fn new(cfg: HomeConfig) -> Self {
        Self {
            cfg,
            failed: None,
            current: None,
            locked_keys: BTreeSet::new(),
            skipped: BTreeSet::new(),
            locked_bytes: Arc::new(HashMap::new()),
            agent_uid: rustix::process::getuid().as_raw(),
        }
    }

    /// The agent runs as another uid than the supervisor (the root supervisor mode).
    pub fn set_agent_uid(&mut self, uid: u32) {
        self.agent_uid = uid;
    }

    fn sync_dir(&self) -> PathBuf {
        self.cfg.root.join(SYNC_DIR)
    }

    fn staged_files(&self) -> PathBuf {
        self.sync_dir().join("files")
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

    /// Applies one `home.sync` part. Part 0 (or a new generation) resets staging.
    pub fn apply_part(&mut self, sync: &HomeSync) -> Result<PartOutcome, HomeError> {
        let gen_file = self.sync_dir().join("generation");
        let staged_gen = fs::read_to_string(&gen_file)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok());
        if sync.part_index == 0 || staged_gen != Some(sync.generation) {
            let _ = fs::remove_dir_all(self.sync_dir());
            fs::create_dir_all(self.staged_files())?;
            fs::write(&gen_file, sync.generation.to_string())?;
            self.failed = None;
            // (P6-MEMLOCK) The locked set belongs to the generation being staged.
            self.locked_keys.clear();
            self.skipped.clear();
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
            if !self.sync_dir().join("manifest.json").exists() {
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
                                    self.skipped.insert(rel);
                                    continue;
                                }
                            }
                        }
                    };
                    // The exec flag travels through staging as the owner-x bit.
                    let staged_mode = if e.exec && !memory { 0o700 } else { 0o600 };
                    // (P6-MEMLOCK) The locked flag travels through staging as a tracked path.
                    if memory && e.locked {
                        self.locked_keys.insert(rel.clone());
                    }
                    match write_atomic(&self.staged_files().join(&rel), &body, staged_mode) {
                        Ok(()) => {
                            self.skipped.remove(&rel);
                        }
                        // A skill file the home cannot hold is skipped, like a helper.
                        Err(error) if !memory && cannot_hold(&error) => {
                            tracing::warn!(
                                path = %e.path.escape_debug(),
                                %error,
                                "skill file not written; skipped"
                            );
                            self.skipped.insert(rel);
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
                            write_atomic(
                                &self.staged_files().join(&r),
                                e.content.unwrap_or_default().as_bytes(),
                                0o600,
                            )?;
                        }
                        _ => {
                            tracing::warn!(path = %e.path.escape_debug(), "app file path refused");
                        }
                    }
                }
            }
            // (P6-TPL-6, C-1143) The helper files: accepted only below `.aurora/helpers/` with
            // the C-1141 rules; a refused entry is a warning, never a write elsewhere. The exec
            // bit travels through staging as the owner-x bit, like a skill file's.
            HomeSyncKind::Helpers => {
                for v in entries {
                    let Some(e) = text_entry(v) else {
                        tracing::warn!("malformed helper file entry refused");
                        continue;
                    };
                    match clean_rel(&e.path) {
                        Some(r) if is_helper_path(&r) => {
                            let staged_mode = if e.exec { 0o700 } else { 0o600 };
                            // One helper the home cannot hold (a file that is also a folder)
                            // is skipped: it must not stall the whole sync.
                            match write_atomic(
                                &self.staged_files().join(&r),
                                e.content.unwrap_or_default().as_bytes(),
                                staged_mode,
                            ) {
                                Ok(()) => {
                                    self.skipped.remove(&r);
                                }
                                Err(error) => {
                                    tracing::warn!(
                                        path = %e.path.escape_debug(),
                                        %error,
                                        "helper file not written; skipped"
                                    );
                                    self.skipped.insert(r);
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
            HomeSyncKind::RenderInputs => {
                let path = self.sync_dir().join("render_inputs.json");
                let mut merged: Map<String, Value> =
                    crate::harvest::read_bounded(&path, STAGED_READ_MAX)
                        .ok()
                        .flatten()
                        .and_then(|b| serde_json::from_slice(&b).ok())
                        .unwrap_or_default();
                for v in entries {
                    if let Value::Object(o) = v {
                        merged.extend(o.clone());
                    }
                }
                write_atomic(&path, &serde_json::to_vec(&merged)?, 0o600)?;
            }
            HomeSyncKind::Tools => {
                let path = self.sync_dir().join("tools.json");
                let mut all: Vec<Value> = crate::harvest::read_bounded(&path, STAGED_READ_MAX)
                    .ok()
                    .flatten()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .unwrap_or_default();
                all.extend(entries.iter().cloned());
                write_atomic(&path, &serde_json::to_vec(&all)?, 0o600)?;
            }
            HomeSyncKind::Manifest => {
                let path = self.sync_dir().join("manifest.json");
                let mut all: Vec<String> = crate::harvest::read_bounded(&path, STAGED_READ_MAX)
                    .ok()
                    .flatten()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .unwrap_or_default();
                for v in entries {
                    if let Ok(m) = serde_json::from_value::<ManifestEntry>(v.clone()) {
                        all.push(m.path);
                    }
                }
                write_atomic(&path, &serde_json::to_vec(&all)?, 0o600)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Installs the staged generation: installs the staged files, prunes managed roots to the
    /// manifest (agent-edited memory files are kept until superseded), renders instructions.
    pub fn commit(&mut self, generation: u64) -> Result<(), HomeError> {
        let root = self.cfg.root.clone();
        let sync = self.sync_dir();
        let manifest: Vec<String> = serde_json::from_slice(&fs::read(sync.join("manifest.json"))?)?;
        let mut keep: BTreeSet<PathBuf> = manifest
            .iter()
            .filter_map(|p| clean_rel(p))
            .filter(|p| self.managed(p))
            .collect();

        let skills_root = self.skills_prefix().map(|s| root.join(s));
        if let (Some(rel), Some(s)) = (self.skills_prefix(), &skills_root) {
            // (SEC-150) The agent's uid owns the home: a link in any component from the home down
            // to the skills root, or anywhere inside it, would let the supervisor's chmod, mkdir
            // and rename write outside the managed tree. Links are removed (never followed) first.
            clear_link_chain(&root, &rel);
            unlink_tree_symlinks(s);
            set_dirs(&root, &rel, 0o755);
        }
        let apps_root = root.join(APPS_DIR);
        clear_link_chain(&root, Path::new(APPS_DIR));
        unlink_tree_symlinks(&apps_root);
        set_dirs(&root, Path::new(APPS_DIR), 0o755);
        // (P6-TPL-6, C-1143) The same link discipline for the helpers root: a link anywhere in
        // the chain or the tree is removed, never followed.
        let helpers_root = root.join(HELPERS_DIR);
        clear_link_chain(&root, Path::new(HELPERS_DIR));
        unlink_tree_symlinks(&helpers_root);
        set_dirs(&root, Path::new(HELPERS_DIR), 0o755);
        // A file that could not be staged goes now, whatever the manifest says.
        for rel in &self.skipped {
            discard(&root, &mut keep, rel);
        }
        // The skills and the helpers are pruned before the install, so that a name can turn
        // from a file into a folder (or back) between two generations.
        if let Some(s) = &skills_root {
            prune_to(&root, s, &keep);
        }
        prune_to(&root, &helpers_root, &keep);

        // Install.
        let mut staged = Vec::new();
        walk_files(&self.staged_files(), &mut staged);
        staged.sort();
        let mut locked_bytes: HashMap<String, Vec<u8>> = HashMap::new();
        for src in staged {
            let Ok(rel) = src.strip_prefix(self.staged_files()) else {
                continue;
            };
            let rel = rel.to_path_buf();
            let dst = root.join(&rel);
            // The staging area is in the agent's home too: only a regular file is installed, and
            // its mode and content are read from the open descriptor.
            let file = match nofollow::open_regular(&src) {
                Ok(f) => f,
                Err(error) => {
                    tracing::warn!(path = %rel.display(), %error, "staged file refused");
                    if is_helper_path(&rel) || self.is_skill_path(&rel) {
                        discard(&root, &mut keep, &rel);
                    }
                    continue;
                }
            };
            let locked = self.locked_keys.contains(&rel);
            let mode = if rel.starts_with(MEMORY_DIR) {
                if rel.file_name().is_some_and(|n| n == INDEX_FILE) || locked {
                    // (P6-MEMLOCK) A locked memory installs read-only, like the index.
                    0o444
                } else {
                    0o644
                }
            } else if file.metadata()?.permissions().mode() & 0o100 != 0 {
                0o555
            } else {
                0o444
            };
            let bytes = match install_staged(&root, file, &src, &dst, mode) {
                Ok(b) => b,
                Err(error) => {
                    if !(is_helper_path(&rel) || self.is_skill_path(&rel) && cannot_hold(&error)) {
                        return Err(error.into());
                    }
                    tracing::warn!(path = %rel.display(), %error, "file not installed; skipped");
                    discard(&root, &mut keep, &rel);
                    continue;
                }
            };
            if locked {
                // The harvest restores these exact (sealed) bytes instead of uploading an edit.
                match rel.file_name().and_then(|n| n.to_str()) {
                    Some(n) => {
                        locked_bytes.insert(n.trim_end_matches(".md").to_owned(), bytes);
                    }
                    None => {
                        tracing::warn!(path = %rel.display(), "locked memory file name refused")
                    }
                }
            }
            keep.insert(rel);
        }

        // Prune the other managed roots.
        // A link at `memory` must not let the prune remove files elsewhere.
        clear_link_chain(&root, Path::new(MEMORY_DIR));
        if let Ok(rd) = fs::read_dir(root.join(MEMORY_DIR)) {
            for e in rd.filter_map(Result::ok) {
                let p = e.path();
                let Ok(rel) = p.strip_prefix(&root) else {
                    continue;
                };
                if !self.managed(rel)
                    || keep.contains(rel)
                    || !e.file_type().is_ok_and(|t| t.is_file())
                {
                    continue;
                }
                let intact = rel.file_name().is_some_and(|n| n == INDEX_FILE)
                    || crate::harvest::read_bounded(&p, crate::harvest::MEMORY_FILE_READ_MAX)
                        .ok()
                        .flatten()
                        .and_then(|b| String::from_utf8(b).ok())
                        .is_some_and(|t| verify(&t));
                if intact {
                    let _ = fs::remove_file(&p);
                }
            }
        }
        if let Some(rel) = self.skills_prefix() {
            lock_tree(&root, &rel, self.agent_uid);
        }
        // The app folders: pruned to the manifest like skills (a revoked grant removes its folder).
        {
            let mut files = Vec::new();
            walk_files(&apps_root, &mut files);
            for f in files {
                if f.strip_prefix(&root).is_ok_and(|r| !keep.contains(r)) {
                    let _ = fs::remove_file(&f);
                }
            }
            let mut dirs = Vec::new();
            walk_dirs(&apps_root, &mut dirs);
            for d in dirs {
                let _ = fs::remove_dir(&d);
            }
            lock_tree(&root, Path::new(APPS_DIR), self.agent_uid);
        }
        // (P6-TPL-6, C-1143) The helper files (pruned to the manifest before the install, so a
        // removed helper is gone): locked read-only (0555/0444 files, 0555 dirs).
        lock_tree(&root, Path::new(HELPERS_DIR), self.agent_uid);

        // .aurora cache + instruction files.
        let aurora = root.join(AURORA_DIR);
        let tools = sync.join("tools.json");
        if tools.exists() {
            fs::rename(&tools, aurora.join("tools.json"))?;
        }
        let ri = sync.join("render_inputs.json");
        if ri.exists() {
            fs::rename(&ri, aurora.join("render_inputs.json"))?;
        }
        self.render_installed()?;
        let _ = fs::remove_dir_all(&sync);
        self.current = Some(generation);
        // (P6-MEMLOCK) What the harvest protects from now on; an empty map unlocks everything.
        self.locked_bytes = Arc::new(locked_bytes);
        Ok(())
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
        let path = self.cfg.root.join(AURORA_DIR).join("render_inputs.json");
        let Ok(Some(raw)) = crate::harvest::read_bounded(&path, RENDER_INPUTS_READ_MAX) else {
            return false;
        };
        serde_json::from_slice::<RenderInputs>(&raw).is_ok_and(|i| {
            crate::instructions::PlatformToolsInfo::on_demand(
                i.platform_tools.as_ref(),
                self.cfg.agent_cli,
            )
        })
    }

    /// (P5-GUIDE-7, C-654) What a harness session captured at its start: the tool descriptors its
    /// bridge lists (the core ones when the bridge is core-only, else all) and the rendered
    /// instruction files. A change of this value is what releases idle sessions; an on-demand
    /// tool changing in a core-only sandbox does not move it. `None` when nothing is installed.
    pub fn catalogue_digest(&self) -> Option<String> {
        use sha2::{Digest, Sha256};
        let aurora = self.cfg.root.join(AURORA_DIR);
        let tools: Vec<Value> =
            crate::harvest::read_bounded(&aurora.join("tools.json"), STAGED_READ_MAX)
                .ok()
                .flatten()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
        let core_only = self.bridge_core_only();
        let listed: Vec<&Value> = tools
            .iter()
            .filter(|t| !core_only || t.get("exposure").and_then(Value::as_str) == Some("core"))
            .collect();
        let mut h = Sha256::new();
        h.update(serde_json::to_vec(&listed).ok()?);
        let mut any = !listed.is_empty();
        for name in &self.cfg.instruction_files {
            let Some(rel) = clean_rel(name) else { continue };
            if let Ok(Some(b)) =
                crate::harvest::read_bounded(&self.cfg.root.join(rel), RENDER_INPUTS_READ_MAX)
            {
                any = true;
                h.update([0u8]);
                h.update(name.as_bytes());
                h.update([0u8]);
                h.update(&b);
            }
        }
        any.then(|| h.finalize().iter().map(|b| format!("{b:02x}")).collect())
    }

    /// Re-renders instruction files from the installed render inputs (boot and every sync).
    pub fn render_installed(&self) -> Result<(), HomeError> {
        let path = self.cfg.root.join(AURORA_DIR).join("render_inputs.json");
        // (FIX-114) Bounded read: the agent's uid owns this directory.
        let raw = match crate::harvest::read_bounded(&path, RENDER_INPUTS_READ_MAX) {
            Ok(Some(b)) => b,
            _ => return Ok(()),
        };
        let inputs: RenderInputs = serde_json::from_slice(&raw)?;
        if let Some(port) = self.cfg.pg {
            // A failure here must not block the sync: the agent only loses `psql service=`.
            if let Err(e) = crate::pg::write(&self.cfg.root, port, &installed_databases(&raw)) {
                tracing::warn!(error = %e, "libpq service file not written");
            }
        }
        // (P5-ICS, D-273) The calendar links the agent may read, as synthetic addresses; removed
        // when there are none. A failure here must not block the sync either.
        if let Err(e) = crate::links::write(&self.cfg.root, &crate::links::installed_links(&raw)) {
            tracing::warn!(error = %e, "links.json not written");
        }
        // (P5-CALDAV, D-275) The servers of the person-pinned apps (addresses only); removed when
        // there are none.
        if let Err(e) =
            crate::servers::write(&self.cfg.root, &crate::servers::installed_servers(&raw))
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
            write_atomic(&self.cfg.root.join(rel), text.as_bytes(), 0o444)?;
        }
        Ok(())
    }

    pub fn current_generation(&self) -> Option<u64> {
        self.current
    }
}

/// Removes a symlink found at any component of `rel` below `root` (`rel` is relative and clean):
/// `root/.claude` as a link, then `root/.claude/skills`, and so on. A real directory or a missing
/// component is left alone. The link is unlinked, its target never touched.
fn clear_link_chain(root: &Path, rel: &Path) {
    let mut p = root.to_path_buf();
    for c in rel.components() {
        p.push(c);
        if fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
            let _ = fs::remove_file(&p);
        }
    }
}

/// Removes every symlink under `dir` (and `dir` itself when it is one), without following any.
/// The agent's uid owns these folders: a link put where Aurora keeps a folder or a file must not
/// be written, chmodded or renamed through.
fn unlink_tree_symlinks(dir: &Path) {
    if fs::symlink_metadata(dir).is_ok_and(|m| m.file_type().is_symlink()) {
        let _ = fs::remove_file(dir);
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.filter_map(Result::ok) {
        let Ok(t) = e.file_type() else { continue };
        if t.is_symlink() {
            let _ = fs::remove_file(e.path());
        } else if t.is_dir() {
            unlink_tree_symlinks(&e.path());
        }
    }
}

/// Sets `mode` on the folder `rel` of the home and on every folder below it; no link, at
/// `rel`, above it or inside it, is followed. A missing `rel` is nothing to do.
fn set_dirs(home: &Path, rel: &Path, mode: u32) {
    if let Ok(dir) = nofollow::open_dir_beneath(home, rel) {
        nofollow::set_tree_modes(dir, mode, None);
    }
}

/// Whether the lock may change a file's mode: it has this one name and is the agent's. A hard
/// link may be another user's file (`fs.protected_hardlinks=0`).
fn may_lock(links: u64, owner: u32, agent: u32) -> bool {
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

        let installed = install_staged(&home, file, &src, &dst, 0o444).unwrap();

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
