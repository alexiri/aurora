//! Package archive reader (09 §2): in-memory gunzip + untar with size
//! accounting, path rules, required files and asset checks.
use std::collections::BTreeMap;
use std::io::Read;

use aurora_core::limits::{PACKAGE_ICON_MAX, PACKAGE_MAX, PACKAGE_SETUP_GUIDE_MAX};
use flate2::read::GzDecoder;
use tar::EntryType;

use crate::Violation;

pub const MANIFEST: &str = "plugin.yaml";
pub const SIGNATURE: &str = "SIGNATURE";
pub const ICON: &str = "assets/icon.png";
pub const ICON_DARK: &str = "assets/icon-dark.png";
pub const GUIDE: &str = "docs/setup.md";
pub const HOOKS_WASM: &str = "hooks.wasm";

/// Size limits applied while reading. `Default` uses the `aurora_core::limits`
/// values; the decompressed cap is 4x the compressed cap (a legitimate package
/// is mostly PNG/WASM which compress poorly, so 4x leaves ample headroom while
/// bounding gzip bombs).
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub package_max: usize,
    pub decompressed_max: usize,
    pub icon_max: usize,
    pub guide_max: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            package_max: PACKAGE_MAX,
            decompressed_max: PACKAGE_MAX.saturating_mul(4),
            icon_max: PACKAGE_ICON_MAX,
            guide_max: PACKAGE_SETUP_GUIDE_MAX,
        }
    }
}

/// A fully-read, structurally valid package. `files` excludes `SIGNATURE`.
#[derive(Debug, Clone)]
pub struct Package {
    pub files: BTreeMap<String, Vec<u8>>,
    pub signature: Option<Vec<u8>>,
}

/// Counts decompressed bytes so a cap hit is reported as oversize, not corruption.
struct Counting<R> {
    inner: R,
    n: std::rc::Rc<std::cell::Cell<u64>>,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let k = self.inner.read(buf)?;
        self.n.set(self.n.get().saturating_add(k as u64));
        Ok(k)
    }
}

fn v(code: &'static str, path: &str, msg: impl Into<String>) -> Violation {
    Violation::new(code, path, msg)
}

fn allowed_path(p: &str) -> bool {
    if matches!(
        p,
        MANIFEST | SIGNATURE | ICON | ICON_DARK | GUIDE | HOOKS_WASM
    ) {
        return true;
    }
    p.strip_prefix("tests/")
        .and_then(|n| n.strip_suffix(".yaml"))
        .is_some_and(|stem| !stem.is_empty() && !stem.contains('/'))
}

/// Returns a violation when the path is unsafe (absolute, traversal, ...).
fn path_safety(p: &str) -> Option<Violation> {
    if p.starts_with('/') {
        return Some(v("path_absolute", p, "absolute path"));
    }
    if p.contains(['\\', '\0']) {
        return Some(v("path_invalid", p, "path contains `\\` or NUL"));
    }
    for c in p.split('/') {
        if c == ".." {
            return Some(v("path_traversal", p, "`..` component"));
        }
        if c.is_empty() || c == "." {
            return Some(v("path_invalid", p, "empty or `.` path component"));
        }
    }
    None
}

impl Package {
    pub fn read(bytes: &[u8], limits: &Limits) -> Result<Self, Vec<Violation>> {
        if bytes.len() > limits.package_max {
            return Err(vec![v(
                "package_too_large",
                "",
                format!("package exceeds {} bytes", limits.package_max),
            )]);
        }
        let cap = limits.decompressed_max as u64;
        let counter = std::rc::Rc::new(std::cell::Cell::new(0u64));
        let gz = Counting {
            inner: GzDecoder::new(bytes).take(cap.saturating_add(1)),
            n: counter.clone(),
        };
        let mut archive = tar::Archive::new(gz);
        let mut out = Vec::new();
        let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut signature = None;
        let mut total: u64 = 0;
        let entries = match archive.entries() {
            Ok(e) => e,
            Err(e) => return Err(vec![v("archive_corrupt", "", e.to_string())]),
        };
        for entry in entries {
            let mut entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    out.push(v("archive_corrupt", "", e.to_string()));
                    break;
                }
            };
            let raw = entry.path_bytes().into_owned();
            let Ok(name) = String::from_utf8(raw) else {
                out.push(v("path_invalid", "", "path is not UTF-8"));
                continue;
            };
            let ty = entry.header().entry_type();
            if ty.is_dir() {
                let d = name.trim_end_matches('/');
                if !matches!(d, "assets" | "docs" | "tests") {
                    out.push(v("path_unknown", &name, "path not allowed in a package"));
                }
                continue;
            }
            if matches!(ty, EntryType::Symlink | EntryType::Link) {
                out.push(v("archive_link", &name, "symlinks/hardlinks are forbidden"));
                continue;
            }
            if !matches!(ty, EntryType::Regular | EntryType::Continuous) {
                out.push(v("archive_entry_type", &name, "unsupported entry type"));
                continue;
            }
            if let Some(x) = path_safety(&name) {
                out.push(x);
                continue;
            }
            if !allowed_path(&name) {
                out.push(v("path_unknown", &name, "path not allowed in a package"));
                continue;
            }
            let size = entry.size();
            total = total.saturating_add(size);
            if total > cap {
                out.push(v(
                    "package_too_large",
                    "",
                    format!("decompressed size exceeds {cap} bytes"),
                ));
                break;
            }
            let mut buf = Vec::with_capacity(usize::try_from(size.min(1 << 20)).unwrap_or(0));
            if let Err(e) = (&mut entry).take(size).read_to_end(&mut buf) {
                out.push(v("archive_corrupt", &name, e.to_string()));
                break;
            }
            let dup = if name == SIGNATURE {
                signature.replace(buf).is_some()
            } else {
                files.insert(name.clone(), buf).is_some()
            };
            if dup {
                out.push(v("path_duplicate", &name, "duplicate entry"));
            }
        }
        if counter.get() > cap {
            out.retain(|x| x.code != "archive_corrupt");
            out.push(v(
                "package_too_large",
                "",
                format!("decompressed size exceeds {cap} bytes"),
            ));
        }
        if !out.is_empty() {
            return Err(out);
        }
        let pkg = Self { files, signature };
        let mut out = Vec::new();
        for req in [MANIFEST, ICON, GUIDE] {
            if !pkg.files.contains_key(req) {
                out.push(v("file_missing", req, "required file is missing"));
            }
        }
        pkg.check_assets(limits, &mut out);
        if out.is_empty() { Ok(pkg) } else { Err(out) }
    }

    pub fn file(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(Vec::as_slice)
    }

    /// `plugin.yaml` as UTF-8.
    pub fn manifest_yaml(&self) -> Option<&str> {
        self.file(MANIFEST)
            .and_then(|b| std::str::from_utf8(b).ok())
    }

    /// Parse the manifest (strict) against the running core version.
    pub fn manifest(&self, running_version: &str) -> Result<crate::Manifest, Vec<Violation>> {
        match self.manifest_yaml() {
            Some(y) => crate::Manifest::parse(y, running_version),
            None => Err(vec![v(
                "manifest_not_utf8",
                MANIFEST,
                "plugin.yaml missing or not UTF-8",
            )]),
        }
    }

    fn check_assets(&self, limits: &Limits, out: &mut Vec<Violation>) {
        for p in [ICON, ICON_DARK] {
            if let Some(b) = self.file(p) {
                check_png(p, b, limits.icon_max, out);
            }
        }
        if let Some(b) = self.file(GUIDE) {
            check_guide(b, limits.guide_max, out);
        }
    }
}

const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

fn check_png(path: &str, b: &[u8], max: usize, out: &mut Vec<Violation>) {
    if b.len() > max {
        out.push(v(
            "icon_too_large",
            path,
            format!("icon exceeds {max} bytes"),
        ));
    }
    let dims = (|| {
        if b.get(..8)? != PNG_SIG || b.get(12..16)? != b"IHDR" {
            return None;
        }
        let w = u32::from_be_bytes(b.get(16..20)?.try_into().ok()?);
        let h = u32::from_be_bytes(b.get(20..24)?.try_into().ok()?);
        Some((w, h))
    })();
    match dims {
        None => out.push(v("icon_not_png", path, "not a PNG image")),
        Some((w, h)) => {
            if w != h {
                out.push(v("icon_not_square", path, format!("icon is {w}x{h}")));
            } else if w < 64 {
                out.push(v(
                    "icon_too_small",
                    path,
                    format!("icon is {w}px, need >= 64"),
                ));
            }
        }
    }
}

fn check_guide(b: &[u8], max: usize, out: &mut Vec<Violation>) {
    if b.len() > max {
        out.push(v(
            "guide_too_large",
            GUIDE,
            format!("guide exceeds {max} bytes"),
        ));
    }
    let Ok(text) = std::str::from_utf8(b) else {
        out.push(v("guide_not_utf8", GUIDE, "guide is not UTF-8"));
        return;
    };
    use pulldown_cmark::{Event, Parser};
    if Parser::new(text).any(|e| matches!(e, Event::Html(_) | Event::InlineHtml(_))) {
        out.push(v(
            "guide_raw_html",
            GUIDE,
            "raw HTML is not allowed in the guide",
        ));
    }
}
