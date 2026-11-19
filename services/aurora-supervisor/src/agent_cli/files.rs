//! (P5-GUIDE-6, SEC-120) Where `aurora-agent` saves what it fetched: `/tmp/aurora-agent/` (0700)
//! unless the agent chose `--out`, and never under a managed root.
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

use super::text::utc_stamp;

pub const DEFAULT_DIR: &str = "/tmp/aurora-agent";
/// Roots the supervisor manages in the agent's home: nothing `aurora-agent` writes goes there.
pub const PROTECTED: [&str; 2] = ["/workspace/.aurora", "/workspace/memory"];

/// Lexical normalisation: no `.`, `..` resolved against what precedes it.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The path with symlinks resolved as far as it exists, the rest appended lexically.
fn resolve(p: &Path) -> PathBuf {
    let mut existing = p.to_path_buf();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(c) = existing.canonicalize() {
            let mut r = c;
            for part in rest.iter().rev() {
                r.push(part);
            }
            return r;
        }
        match (
            existing.file_name().map(ToOwned::to_owned),
            existing.parent(),
        ) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent.to_path_buf();
            }
            _ => return p.to_path_buf(),
        }
    }
}

fn under(p: &Path, root: &Path) -> bool {
    p == root || p.starts_with(root)
}

/// Refuses an `--out` under a protected root, whether written directly, with `..`, or through a
/// symlink; returns the absolute path to write.
pub fn check_out(out: &Path, cwd: &Path, protected: &[PathBuf]) -> Result<PathBuf, String> {
    let abs = normalize(&if out.is_absolute() {
        out.to_path_buf()
    } else {
        cwd.join(out)
    });
    let real = resolve(&abs);
    for root in protected {
        let real_root = resolve(root);
        if under(&abs, root)
            || under(&real, root)
            || under(&real, &real_root)
            || under(&abs, &real_root)
        {
            return Err(format!(
                "--out {}: the Aurora-managed folders {} cannot be written by aurora-agent",
                out.display(),
                protected
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(" and ")
            ));
        }
    }
    if abs.is_dir() {
        return Err(format!("--out {}: is a directory", out.display()));
    }
    Ok(abs)
}

fn ensure_dir(dir: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(dir) {
        Ok(m) if m.is_dir() => Ok(()),
        Ok(_) => Err(std::io::Error::other(format!(
            "{} exists and is not a directory",
            dir.display()
        ))),
        Err(_) => {
            let mut b = std::fs::DirBuilder::new();
            b.recursive(true).mode(0o700);
            b.create(dir)
        }
    }
}

fn clean(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

#[must_use]
pub fn extension(content_type: &str, json_ok: bool) -> &'static str {
    let ct = content_type.to_ascii_lowercase();
    if json_ok || ct.contains("json") {
        "json"
    } else if ct.starts_with("text/") || ct.contains("xml") || ct.contains("yaml") {
        "txt"
    } else {
        "bin"
    }
}

/// Saves `bytes` and returns the path. With `out` the file is exactly there (overwritten, never
/// through a symlink); otherwise a new `<app>-<operation>-<UTC timestamp>.<ext>` under `dir`.
pub fn save(
    bytes: &[u8],
    out: Option<&Path>,
    dir: &Path,
    app: &str,
    operation: &str,
    ext: &str,
) -> Result<PathBuf, String> {
    let io = |e: std::io::Error, p: &Path| format!("cannot write {}: {e}", p.display());
    if let Some(p) = out {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io(e, parent))?;
        }
        if std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!("--out {}: is a symlink", p.display()));
        }
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(p)
            .map_err(|e| io(e, p))?;
        f.write_all(bytes).map_err(|e| io(e, p))?;
        return Ok(p.to_path_buf());
    }
    ensure_dir(dir).map_err(|e| io(e, dir))?;
    let stem = format!("{}-{}-{}", clean(app), clean(operation), utc_stamp());
    for n in 1..100 {
        let name = if n == 1 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem}-{n}.{ext}")
        };
        let p = dir.join(name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&p)
        {
            Ok(mut f) => {
                f.write_all(bytes).map_err(|e| io(e, &p))?;
                return Ok(p);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io(e, &p)),
        }
    }
    Err(format!(
        "cannot find a free file name under {}",
        dir.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("aurora-agent-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn out_under_a_protected_root_is_refused_however_it_is_spelled() {
        let base = scratch("protected");
        let managed = base.join("home/.aurora");
        let memory = base.join("home/memory");
        std::fs::create_dir_all(managed.join("apps")).unwrap();
        std::fs::create_dir_all(&memory).unwrap();
        std::os::unix::fs::symlink(&managed, base.join("alias")).unwrap();
        let protected = vec![managed.clone(), memory.clone()];
        for bad in [
            managed.join("apps/x/api.json"),
            managed.join("new.json"),
            memory.join("note.md"),
            base.join("home/other/../.aurora/x"),
            base.join("alias/apps/x"),
            base.join("alias/not-yet/x"),
        ] {
            assert!(
                check_out(&bad, &base, &protected).is_err(),
                "{}",
                bad.display()
            );
        }
        assert!(check_out(&base.join("home/ok.json"), &base, &protected).is_ok());
        assert!(check_out(Path::new("rel.json"), &base, &protected).is_ok());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_default_directory_is_private_and_names_do_not_collide() {
        let base = scratch("save");
        let dir = base.join("out");
        let a = save(b"{}", None, &dir, "app", "op/../x", "json").unwrap();
        let b = save(b"{}", None, &dir, "app", "op/../x", "json").unwrap();
        assert_ne!(a, b);
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&a).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(
            a.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("app-op____x-")
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn out_never_writes_through_a_symlink() {
        let base = scratch("symlink");
        let target = base.join("target");
        std::fs::write(&target, "keep").unwrap();
        let link = base.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(save(b"x", Some(&link), &base, "a", "b", "json").is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
        let _ = std::fs::remove_dir_all(&base);
    }
}
