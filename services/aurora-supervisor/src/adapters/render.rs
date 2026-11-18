//! What the phase-5 adapters (`pi`, `goose`, `opencode`) share for files they render and state they
//! keep (`docs/12-supervisor-harness.md` §Adapters `pi`, `goose` and `opencode`, D-362(b), SEC-133).
//!
//! Layout, under the sandbox home (the supervisor's working directory):
//! `.aurora/harness/<id>/config/` (rendered by Aurora, re-rendered and read back before every start)
//! and `.aurora/harness/<id>/sessions/<session ref>/` (the harness's own state of one conversation;
//! purging a session removes exactly this directory).

use std::io::Write as _;
use std::path::{Path, PathBuf};

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

/// Removes a file, a link or a directory tree; a missing path is success.
pub fn remove_any(p: &Path) -> std::io::Result<()> {
    let r = match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(p),
        Ok(_) => std::fs::remove_file(p),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    match r {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Writes `content` to `path`, replacing whatever is there (a read-only file, a link, a
/// directory), creating the new file exclusively so a link planted in between is not followed, and
/// reads it back: a file that is not exactly what was rendered is an error (SEC-66, SEC-133).
pub fn write_exclusive(path: &Path, content: &str) -> std::io::Result<()> {
    remove_any(path)?;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    f.write_all(content.as_bytes())?;
    f.sync_all()?;
    drop(f);
    if std::fs::read_to_string(path)? != content {
        return Err(std::io::Error::other(format!(
            "{} changed while written",
            path.display()
        )));
    }
    Ok(())
}

/// Creates `dir` (and parents) if it is a directory or missing; anything else in its place
/// (a file or link planted by the agent) is replaced by a directory.
pub fn ensure_dir(dir: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(dir) {
        Ok(m) if m.is_dir() => Ok(()),
        Ok(_) => {
            remove_any(dir)?;
            std::fs::create_dir_all(dir)
        }
        Err(_) => std::fs::create_dir_all(dir),
    }
}

/// Renders every `(file name, content)` of `files` into `dir`, then removes each name in
/// `forbidden` (the harness's credential stores).
pub fn render_dir(dir: &Path, files: &[(&str, String)], forbidden: &[&str]) -> std::io::Result<()> {
    ensure_dir(dir)?;
    for (name, content) in files {
        write_exclusive(&dir.join(name), content)?;
    }
    for name in forbidden {
        remove_any(&dir.join(name))?;
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
        ensure_dir(&d.join("cfg")).unwrap();
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
