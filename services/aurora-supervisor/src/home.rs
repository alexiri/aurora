//! Home projection (`docs/12-supervisor-harness.md` §Home projection / §Sync protocol).
//!
//! Parts of one generation are staged on disk under `.aurora/sync/` (never buffered in memory) and
//! idempotently overwritten on replay. Only [`HomeProjector::commit`] touches the live projection:
//! staged files are renamed into place, managed roots are pruned to the manifest and instruction
//! files are re-rendered. A crash before commit leaves the previous projection intact; the next
//! generation (or a restart of the same one at part 0) wipes the stale staging area.
//!
//! Entry shapes (the wire leaves them untyped, so they are defined here):
//! - `skills`: `{path, content}`, `path` inside the harness skills dir (files become 0444).
//! - `memory`: `{path: "memory/<key>.md", content}` (supervisor appends the checksum trailer; 0644)
//!   and `{path: "memory/index.md", content}` (verbatim, 0444).
//! - `render_inputs`: objects whose keys are [`RenderInputs`] fields; merged across entries.
//! - `skills` entries: `{path, content, exec?}`; `exec: true` installs the file 0555 instead of 0444.
//! - `tools`: opaque catalogue entries, stored as `.aurora/tools.json`.
//! - `manifest`: `{path}`; every managed path that should exist.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Component, Path, PathBuf};

use aurora_protocol::runner::{HomeSync, HomeSyncKind};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::harvest::{INDEX_FILE, MEMORY_DIR, seal, verify};
use crate::instructions::{RenderInputs, render_with};
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
    #[serde(default)]
    content: String,
    /// Skill files only: install as 0555 instead of 0444 (`14` skills, executable bit).
    #[serde(default)]
    exec: bool,
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
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let tmp = parent.join(format!(".{name}.aurora-tmp"));
    let _ = fs::remove_file(&tmp);
    let mut f = fs::File::create(&tmp)?;
    f.write_all(bytes)?;
    f.set_permissions(fs::Permissions::from_mode(mode))?;
    f.sync_all()?;
    fs::rename(&tmp, path)
}

/// (C-643) Root of the app folders in the home.
pub const APPS_DIR: &str = ".aurora/apps";

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
        }
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

    /// A managed projection path: under the skills dir, `memory/<name>.md` or an app folder file
    /// `.aurora/apps/<id>/<name>` (C-643).
    fn managed(&self, rel: &Path) -> bool {
        if is_app_path(rel) {
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
                                && memory == r.starts_with(MEMORY_DIR) =>
                        {
                            r
                        }
                        _ => {
                            tracing::warn!(path = %e.path.escape_debug(), "path outside managed roots refused");
                            continue;
                        }
                    };
                    let body = if memory && rel.file_name().is_some_and(|n| n != INDEX_FILE) {
                        seal(&e.content)
                    } else {
                        e.content
                    };
                    // The exec flag travels through staging as the owner-x bit.
                    let staged_mode = if e.exec && !memory { 0o700 } else { 0o600 };
                    write_atomic(
                        &self.staged_files().join(&rel),
                        body.as_bytes(),
                        staged_mode,
                    )?;
                }
            }
            HomeSyncKind::Apps => {
                for v in entries {
                    let Ok(e) = serde_json::from_value::<FileEntry>(v.clone()) else {
                        tracing::warn!("malformed app file entry refused");
                        continue;
                    };
                    match clean_rel(&e.path) {
                        Some(r) if is_app_path(&r) => {
                            write_atomic(
                                &self.staged_files().join(&r),
                                e.content.as_bytes(),
                                0o600,
                            )?;
                        }
                        _ => {
                            tracing::warn!(path = %e.path.escape_debug(), "app file path refused");
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

    /// Installs the staged generation: renames staged files into place, prunes managed roots to the
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
            set_dirs(s, 0o755);
        }
        let apps_root = root.join(APPS_DIR);
        clear_link_chain(&root, Path::new(APPS_DIR));
        unlink_tree_symlinks(&apps_root);
        set_dirs(&apps_root, 0o755);

        // Install.
        let mut staged = Vec::new();
        walk_files(&self.staged_files(), &mut staged);
        staged.sort();
        for src in staged {
            let Ok(rel) = src.strip_prefix(self.staged_files()) else {
                continue;
            };
            let rel = rel.to_path_buf();
            let dst = root.join(&rel);
            let mode = if rel.starts_with(MEMORY_DIR) {
                if rel.file_name().is_some_and(|n| n == INDEX_FILE) {
                    0o444
                } else {
                    0o644
                }
            } else if fs::metadata(&src)?.permissions().mode() & 0o100 != 0 {
                0o555
            } else {
                0o444
            };
            if let Some(p) = dst.parent() {
                // Every managed destination: no link in the chain from the home down.
                if let Ok(rp) = p.strip_prefix(&root) {
                    clear_link_chain(&root, rp);
                }
                fs::create_dir_all(p)?;
            }
            fs::set_permissions(&src, fs::Permissions::from_mode(mode))?;
            fs::rename(&src, &dst)?;
            keep.insert(rel);
        }

        // Prune managed roots.
        if let Some(s) = &skills_root {
            let mut files = Vec::new();
            walk_files(s, &mut files);
            for f in files {
                if f.strip_prefix(&root).is_ok_and(|r| !keep.contains(r)) {
                    let _ = fs::remove_file(&f);
                }
            }
            let mut dirs = Vec::new();
            walk_dirs(s, &mut dirs);
            for d in dirs {
                let _ = fs::remove_dir(&d); // only succeeds when empty
            }
        }
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
        if let Some(s) = &skills_root {
            lock_tree(s);
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
            if apps_root.is_dir() {
                lock_tree(&apps_root);
            }
        }

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
        Ok(())
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

fn set_dirs(root: &Path, mode: u32) {
    let mut dirs = Vec::new();
    walk_dirs(root, &mut dirs);
    dirs.push(root.to_path_buf());
    for d in dirs {
        let _ = fs::set_permissions(&d, fs::Permissions::from_mode(mode));
    }
}

fn lock_tree(root: &Path) {
    let mut files = Vec::new();
    walk_files(root, &mut files);
    for f in files {
        // Executable skill files stay 0555; everything else is 0444.
        let exec = fs::metadata(&f).is_ok_and(|m| m.permissions().mode() & 0o100 != 0);
        let _ = fs::set_permissions(
            &f,
            fs::Permissions::from_mode(if exec { 0o555 } else { 0o444 }),
        );
    }
    set_dirs(root, 0o555);
}
