//! The boot document (`docs/11-runner-sandboxes.md` §The boot document).
//!
//! `std::env::remove_var` is `unsafe` in edition 2024 and this crate forbids unsafe code, so the
//! variable is not removed from the process environment; instead the process is made
//! non-dumpable (same-uid processes cannot read `/proc/self/environ`) and every child is spawned
//! through [`crate::spawn`], which removes it. The supervisor never passes it on.

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use aurora_protocol::common::SecretString;
use base64::Engine as _;
use serde::Deserialize;
use uuid::Uuid;

pub const BOOT_ENV: &str = "AURORA_BOOT";
pub const MAX_BOOT_BYTES: usize = 64 * 1024;
pub const SENTINEL: &str = "aurora-managed";
pub const DEFAULT_CA_PATH: &str = "/run/aurora/ca.pem";
/// Boot `files` are confined to the sandbox home.
pub const HOME_DIR: &str = "/workspace";

#[derive(Debug, thiserror::Error)]
pub enum BootError {
    #[error("{BOOT_ENV} is not set")]
    Missing,
    #[error("boot document too large")]
    TooLarge,
    #[error("boot document is not base64url: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("boot document is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid boot document: {0}")]
    Invalid(&'static str),
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct BootAgent {
    pub id: Uuid,
    pub handle: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BootGateway {
    /// `host:port` of the gateway proxy.
    pub proxy: String,
    pub boot_token: SecretString,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BootFile {
    pub path: PathBuf,
    #[serde(default)]
    pub mode: Option<String>,
    pub content_b64: String,
    #[serde(default)]
    pub only_if_absent_or_sentinel: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BootHarness {
    pub id: String,
    #[serde(default)]
    pub capabilities_expected: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BootDocument {
    pub v: u32,
    pub sandbox_id: Uuid,
    pub boot_id: Uuid,
    pub agent: BootAgent,
    pub runner_url: String,
    pub bootstrap_token: SecretString,
    pub gateway: BootGateway,
    pub ca_pem: String,
    #[serde(default)]
    pub files: Vec<BootFile>,
    pub harness: BootHarness,
    #[serde(default)]
    pub timezone: Option<String>,
}

impl BootDocument {
    /// Decode and validate the raw (base64url JSON) document.
    pub fn parse(raw: &str) -> Result<Self, BootError> {
        let raw = raw.trim();
        if raw.len() > MAX_BOOT_BYTES {
            return Err(BootError::TooLarge);
        }
        let bytes =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(raw.trim_end_matches('='))?;
        let doc: Self = serde_json::from_slice(&bytes)?;
        doc.validate()?;
        Ok(doc)
    }

    fn validate(&self) -> Result<(), BootError> {
        if self.v != 1 {
            return Err(BootError::Invalid("unsupported version"));
        }
        if !self.runner_url.starts_with("ws://") && !self.runner_url.starts_with("wss://") {
            return Err(BootError::Invalid("runner_url must be ws:// or wss://"));
        }
        if !self.gateway.proxy.contains(':') {
            return Err(BootError::Invalid("gateway.proxy must be host:port"));
        }
        if self.bootstrap_token.expose().is_empty() || self.gateway.boot_token.expose().is_empty() {
            return Err(BootError::Invalid("missing token"));
        }
        for f in &self.files {
            if !f.path.is_absolute() {
                return Err(BootError::Invalid("file path must be absolute"));
            }
            if f.path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(BootError::Invalid("file path must not contain `..`"));
            }
        }
        Ok(())
    }
}

/// Read `AURORA_BOOT` from the environment.
pub fn read_boot_env() -> Result<String, BootError> {
    std::env::var(BOOT_ENV).map_err(|_| BootError::Missing)
}

/// `prctl(PR_SET_DUMPABLE, 0)`.
pub fn make_non_dumpable() -> std::io::Result<()> {
    rustix::process::set_dumpable_behavior(rustix::process::DumpableBehavior::NotDumpable)?;
    Ok(())
}

/// Whether the process is currently dumpable.
pub fn is_dumpable() -> std::io::Result<bool> {
    Ok(rustix::process::dumpable_behavior()? != rustix::process::DumpableBehavior::NotDumpable)
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> BootError + '_ {
    move |source| BootError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn write_file(path: &Path, content: &[u8], mode: u32) -> Result<(), BootError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io_err(parent))?;
    }
    let mut f = std::fs::File::create(path).map_err(io_err(path))?;
    f.set_permissions(std::fs::Permissions::from_mode(mode))
        .map_err(io_err(path))?;
    f.write_all(content).map_err(io_err(path))
}

/// Refuse `path` unless it is inside `home` (lexically, and after resolving the deepest existing
/// ancestor, so a symlink inside the home cannot lead out of it) and is not itself a symlink.
fn confine(path: &Path, home: &Path) -> Result<(), BootError> {
    let escape = || BootError::Invalid("file path escapes the home");
    if !path.starts_with(home)
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(escape());
    }
    let real_home = home.canonicalize().map_err(io_err(home))?;
    let mut anc = path.parent();
    while let Some(a) = anc {
        if a.symlink_metadata().is_ok() {
            let real = a.canonicalize().map_err(io_err(a))?;
            if !real.starts_with(&real_home) {
                return Err(escape());
            }
            break;
        }
        anc = a.parent();
    }
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(escape());
    }
    Ok(())
}

/// Write boot `files` (confined to [`HOME_DIR`]) and the CA bundle. Returns the paths skipped
/// because they exist without the `aurora-managed` sentinel (`only_if_absent_or_sentinel`).
pub fn write_boot_files(doc: &BootDocument, ca_path: &Path) -> Result<Vec<PathBuf>, BootError> {
    write_boot_files_in(doc, ca_path, Path::new(HOME_DIR))
}

/// [`write_boot_files`] with an explicit home directory.
pub fn write_boot_files_in(
    doc: &BootDocument,
    ca_path: &Path,
    home: &Path,
) -> Result<Vec<PathBuf>, BootError> {
    // Validate every path before writing anything.
    for f in &doc.files {
        confine(&f.path, home)?;
    }
    write_file(ca_path, doc.ca_pem.as_bytes(), 0o644)?;
    let mut skipped = Vec::new();
    for f in &doc.files {
        let content = base64::engine::general_purpose::STANDARD
            .decode(&f.content_b64)
            .map_err(BootError::Base64)?;
        if f.only_if_absent_or_sentinel
            && let Ok(existing) = std::fs::read(&f.path)
            && !contains(&existing, SENTINEL.as_bytes())
        {
            skipped.push(f.path.clone());
            continue;
        }
        let mode = match &f.mode {
            Some(m) => u32::from_str_radix(m.trim_start_matches("0o"), 8)
                .map_err(|_| BootError::Invalid("bad file mode"))?,
            None => 0o644,
        };
        write_file(&f.path, &content, mode)?;
    }
    Ok(skipped)
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}
