//! What the phase-5 adapters (`pi`, `goose`, `opencode`) share for files they render and state they
//! keep (`docs/12-supervisor-harness.md` §Adapters `pi`, `goose` and `opencode`, D-362(b), SEC-133).
//!
//! Layout, under the sandbox home (the supervisor's working directory):
//! `.aurora/harness/<id>/config/` (rendered by Aurora, re-rendered and read back before every start)
//! and `.aurora/harness/<id>/sessions/<session ref>/` (the harness's own state of one conversation;
//! purging a session removes exactly this directory).

use std::io::{Read as _, Write as _};
use std::os::fd::AsFd as _;
use std::path::{Path, PathBuf};

use rustix::fs::{Mode, OFlags};

use crate::nofollow;

/// `<home>/.aurora/harness/<id>`.
pub fn harness_dir(home: &Path, id: &str) -> PathBuf {
    home.join(".aurora").join("harness").join(id)
}

/// `<home>/.aurora/harness/<id>/config`.
pub fn config_dir(home: &Path, id: &str) -> PathBuf {
    harness_dir(home, id).join("config")
}

/// `<home>/.aurora/harness/<id>/sessions/<session ref>`. The caller has checked the reference with
/// [`safe_ref`].
pub fn session_dir(home: &Path, id: &str, session_ref: &str) -> PathBuf {
    harness_dir(home, id).join("sessions").join(session_ref)
}

/// A session reference names a directory and becomes an argv word: a short path-safe token that
/// cannot be read as an option.
pub fn safe_ref(r: &str) -> bool {
    crate::harness::is_safe_session_ref(r) && !r.starts_with('-')
}

/// `path` relative to `home`. Everything below the home is the agent's: the functions here work
/// from the home's descriptor and follow no link on the way.
fn below<'a>(home: &Path, path: &'a Path) -> std::io::Result<&'a Path> {
    path.strip_prefix(home)
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))
}

/// Removes a file, a link or a directory tree at `path` below `home`; a missing path is success,
/// and so is one whose folder is not there (a link in its place is not a folder).
pub fn remove_any(home: &Path, path: &Path) -> std::io::Result<()> {
    nofollow::remove_tree_beneath(home, below(home, path)?)
}

/// Writes `content` to `path` below `home`, replacing whatever is there (a read-only file, a
/// link, a directory), creating the new file exclusively so a link planted in between is not
/// followed, and reads it back: a file that is not exactly what was rendered is an error (SEC-66,
/// SEC-133). The folder must exist ([`ensure_dir`]).
pub fn write_exclusive(home: &Path, path: &Path, content: &str) -> std::io::Result<()> {
    let rel = below(home, path)?;
    let (parent, name) = rel
        .parent()
        .zip(rel.file_name())
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    nofollow::remove_tree_beneath(home, rel)?;
    let dir = nofollow::open_dir_beneath(home, parent)?;
    let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut f = std::fs::File::from(rustix::fs::openat(
        &dir,
        name,
        flags,
        Mode::from_raw_mode(0o666),
    )?);
    f.write_all(content.as_bytes())?;
    f.sync_all()?;
    drop(f);
    let mut back = Vec::new();
    nofollow::open_regular_at(dir.as_fd(), name)?
        .take(content.len() as u64 + 1)
        .read_to_end(&mut back)?;
    if back != content.as_bytes() {
        return Err(std::io::Error::other(format!(
            "{} changed while written",
            path.display()
        )));
    }
    Ok(())
}

/// Makes the folder `dir` below `home` (and its parents); anything but a directory in the place
/// of one of them (a file or link planted by the agent) is replaced by a directory.
pub fn ensure_dir(home: &Path, dir: &Path) -> std::io::Result<()> {
    nofollow::replace_dir_beneath(home, below(home, dir)?).map(drop)
}

/// Renders every `(relative path, content)` of `files` into `dir` below `home` (creating the
/// directories in between), then removes each relative path in `forbidden` (the harness's
/// credential stores).
pub fn render_dir(
    home: &Path,
    dir: &Path,
    files: &[(&str, String)],
    forbidden: &[&str],
) -> std::io::Result<()> {
    ensure_dir(home, dir)?;
    for (name, content) in files {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            ensure_dir(home, parent)?;
        }
        write_exclusive(home, &path, content)?;
    }
    for name in forbidden {
        remove_any(home, &dir.join(name))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("render-{name}-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn render_replaces_planted_files_links_and_directories_and_removes_credentials() {
        let d = tmp("replace");
        let cfg = d.join("cfg");
        std::fs::create_dir_all(&cfg).unwrap();
        // A link the agent planted where a rendered file goes, a directory where another goes,
        // and a credential store.
        let secret = d.join("elsewhere");
        std::fs::write(&secret, "keep me").unwrap();
        std::os::unix::fs::symlink(&secret, cfg.join("a.json")).unwrap();
        std::fs::create_dir_all(cfg.join("b.json")).unwrap();
        std::fs::write(cfg.join("auth.json"), r#"{"key":"sk-planted"}"#).unwrap();
        render_dir(
            &d,
            &cfg,
            &[("a.json", "{\"a\":1}".into()), ("b.json", "{}".into())],
            &["auth.json"],
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(cfg.join("a.json")).unwrap(),
            "{\"a\":1}"
        );
        assert_eq!(
            std::fs::read_to_string(&secret).unwrap(),
            "keep me",
            "the link was not followed"
        );
        assert_eq!(std::fs::read_to_string(cfg.join("b.json")).unwrap(), "{}");
        assert!(!cfg.join("auth.json").exists());
    }

    #[test]
    fn a_directory_replaced_by_a_file_is_recreated_and_refs_are_checked() {
        let d = tmp("dir");
        std::fs::write(d.join("cfg"), "not a dir").unwrap();
        ensure_dir(&d, &d.join("cfg")).unwrap();
        assert!(d.join("cfg").is_dir());
        assert!(safe_ref("0192f3a0-aaaa-7bbb-8ccc-ddddeeeeffff"));
        for bad in ["", "-x", "../x", "a/b", "a b"] {
            assert!(!safe_ref(bad), "{bad}");
        }
        assert_eq!(
            session_dir(Path::new("/w"), "pi", "r1"),
            Path::new("/w/.aurora/harness/pi/sessions/r1")
        );
    }
}
