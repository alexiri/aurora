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
use crate::instructions::{RenderInputs, render};

const AURORA_DIR: &str = ".aurora";
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

    /// A managed projection path: under the skills dir, or `memory/<name>.md`.
    fn managed(&self, rel: &Path) -> bool {
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
                        Some(r) if self.managed(&r) && memory == r.starts_with(MEMORY_DIR) => r,
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
            HomeSyncKind::RenderInputs => {
                let path = self.sync_dir().join("render_inputs.json");
                let mut merged: Map<String, Value> = fs::read(&path)
                    .ok()
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
                let mut all: Vec<Value> = fs::read(&path)
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .unwrap_or_default();
                all.extend(entries.iter().cloned());
                write_atomic(&path, &serde_json::to_vec(&all)?, 0o600)?;
            }
            HomeSyncKind::Manifest => {
                let path = self.sync_dir().join("manifest.json");
                let mut all: Vec<String> = fs::read(&path)
                    .ok()
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
        if let Some(s) = &skills_root {
            set_dirs(s, 0o755);
        }

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
                    || fs::read_to_string(&p).is_ok_and(|t| verify(&t));
                if intact {
                    let _ = fs::remove_file(&p);
                }
            }
        }
        if let Some(s) = &skills_root {
            lock_tree(s);
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

    /// Re-renders instruction files from the installed render inputs (boot and every sync).
    pub fn render_installed(&self) -> Result<(), HomeError> {
        let path = self.cfg.root.join(AURORA_DIR).join("render_inputs.json");
        let inputs: RenderInputs = match fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b)?,
            Err(_) => return Ok(()),
        };
        let text = render(&inputs);
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
