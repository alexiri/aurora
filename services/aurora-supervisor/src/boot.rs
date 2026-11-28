//! The boot document (`docs/11-runner-sandboxes.md` §The boot document).
//!
//! `std::env::remove_var` is `unsafe` in edition 2024 and this crate forbids unsafe code, so the
//! variable is not removed from the process environment; instead the process is made
//! non-dumpable (same-uid processes cannot read `/proc/self/environ`) and every child is spawned
//! through [`crate::spawn`], which removes it. The supervisor never passes it on.

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
    /// (phase 1, RFE-002) Always `true`: the gateway proxy listener is TLS-only; any other value
    /// (or a missing field) is refused at start.
    #[serde(default)]
    pub tls: bool,
    /// Name the gateway's proxy certificate is verified against; defaults to the host part of
    /// `proxy`, which must be in the certificate's SAN set.
    #[serde(default)]
    pub server_name: Option<String>,
    pub boot_token: SecretString,
}

impl BootGateway {
    /// The verification name: `server_name`, else the host part of `proxy`.
    pub fn verify_name(&self) -> String {
        if let Some(n) = self.server_name.as_deref().filter(|n| !n.is_empty()) {
            return n.to_owned();
        }
        let host = self
            .proxy
            .rsplit_once(':')
            .map_or(self.proxy.as_str(), |(h, _)| h);
        host.trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned()
    }
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
    /// (FIX-138 item 1b) Harness ids the instance enables (`hosted.enabled_harnesses`); the
    /// terminal's `claude`/`codex` wrappers refuse a harness not listed. Absent (an older API):
    /// only the booted harness.
    #[serde(default)]
    pub enabled_harnesses: Vec<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    /// (RFE-003) What the sandbox may do beyond ordinary traffic; phase 1: `browser`.
    /// Absent = none, so nothing listens on the open proxy port.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

impl BootDocument {
    pub fn has_capability(&self, cap: &str) -> bool {
        self.capabilities.iter().any(|c| c == cap)
    }

    /// The harnesses the terminal's wrappers may start: the document's list, else the booted one.
    pub fn terminal_harnesses(&self) -> Vec<String> {
        if self.enabled_harnesses.is_empty() {
            vec![self.harness.id.clone()]
        } else {
            self.enabled_harnesses.clone()
        }
    }

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
        // (FIX-114, docs/16 §W6) The supervisor cannot dial `wss://` (no TLS feature in the
        // WebSocket client): refuse it at boot with a clear error instead of failing to connect.
        if self.runner_url.starts_with("wss://") {
            return Err(BootError::Invalid(
                "runner_url must be ws:// (wss:// is not supported until TLS is implemented)",
            ));
        }
        if !self.runner_url.starts_with("ws://") {
            return Err(BootError::Invalid("runner_url must be ws://"));
        }
        if !self.gateway.proxy.contains(':') {
            return Err(BootError::Invalid("gateway.proxy must be host:port"));
        }
        if !self.gateway.tls {
            return Err(BootError::Invalid(
                "gateway.tls must be true: the gateway proxy listener is TLS-only",
            ));
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

/// Replaces `rel` below `base` (a temp file and a rename): what was at the name, a FIFO or a
/// hard link among them, is never opened, and no link below `base` is followed. A replaced
/// file's owner is kept.
fn write_file(base: &Path, rel: &Path, content: &[u8], mode: u32) -> Result<(), BootError> {
    crate::nofollow::rewrite_beneath(base, rel, content, mode).map_err(io_err(&base.join(rel)))
}

/// `path` (one [`confine`] passed) below the real home, with the links among its folders
/// resolved: a folder the agent linked to another folder of its home stays a link.
fn resolved(path: &Path, home: &Path) -> Result<(PathBuf, PathBuf), BootError> {
    let escape = || BootError::Invalid("file path escapes the home");
    let real_home = home.canonicalize().map_err(io_err(home))?;
    let parent = path.parent().ok_or_else(escape)?;
    let existing = parent
        .ancestors()
        .find(|a| a.symlink_metadata().is_ok())
        .ok_or_else(escape)?;
    let rel = existing
        .canonicalize()
        .map_err(io_err(existing))?
        .join(parent.strip_prefix(existing).map_err(|_| escape())?)
        .join(path.file_name().ok_or_else(escape)?)
        .strip_prefix(&real_home)
        .map_err(|_| escape())?
        .to_path_buf();
    Ok((real_home, rel))
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
    // The bundle's own folder is the base: it is not below the home.
    let (Some(ca_dir), Some(ca_name)) = (ca_path.parent(), ca_path.file_name()) else {
        return Err(BootError::Invalid("bad CA bundle path"));
    };
    write_file(ca_dir, Path::new(ca_name), doc.ca_pem.as_bytes(), 0o644)?;
    let mut skipped = Vec::new();
    for f in &doc.files {
        let content = base64::engine::general_purpose::STANDARD
            .decode(&f.content_b64)
            .map_err(BootError::Base64)?;
        if f.only_if_absent_or_sentinel && !replaceable(&f.path) {
            skipped.push(f.path.clone());
            continue;
        }
        let mode = match &f.mode {
            Some(m) => u32::from_str_radix(m.trim_start_matches("0o"), 8)
                .map_err(|_| BootError::Invalid("bad file mode"))?,
            None => 0o644,
        };
        let (base, rel) = resolved(&f.path, home)?;
        write_file(&base, &rel, &content, mode)?;
    }
    Ok(skipped)
}

/// Largest file read to look for the sentinel.
const MANAGED_READ_MAX: u64 = 4 * 1024 * 1024;

/// A file that is not there, or one that carries the sentinel. The home outlives a boot and is
/// the agent's: a FIFO, a link or a larger file is left as it is, and never waited on.
fn replaceable(path: &Path) -> bool {
    match crate::harvest::read_bounded(path, MANAGED_READ_MAX) {
        Ok(Some(existing)) => contains(&existing, SENTINEL.as_bytes()),
        Ok(None) => false,
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}
