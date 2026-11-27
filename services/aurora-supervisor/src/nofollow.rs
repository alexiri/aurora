//! File access in directories the agent's uid owns. The agent can put a link, or a FIFO, at any
//! name there, at any time: nothing here follows a link in the last path component, and modes are
//! set on open descriptors, never by path. The directories above are the caller's to check.

use std::fs;
use std::io::{self, Write as _};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::Path;

use rustix::fs::OFlags;
use uuid::Uuid;

/// Opens an existing regular file for reading. A link is an error, and so is anything that is not
/// a regular file; a FIFO does not block the open.
pub(crate) fn open_regular(path: &Path) -> io::Result<fs::File> {
    let flags = OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags.bits() as i32)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok(file)
}

/// `fchmod` on the descriptor.
pub(crate) fn set_mode(file: &fs::File, mode: u32) -> io::Result<()> {
    file.set_permissions(fs::Permissions::from_mode(mode))
}

/// Replaces `path` with `bytes` at `mode`: a new temp file beside it (random name, created
/// exclusively, so nothing planted is ever opened), then a rename over `path`. The temp file is
/// removed when any step fails.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    // Not built from the file's name, which may already be as long as a name can be.
    let tmp = parent.join(format!(".{}.aurora-tmp", Uuid::now_v7().simple()));
    // `create_new` is `O_CREAT | O_EXCL`, which never follows a link.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(OFlags::NOFOLLOW.bits() as i32)
        .open(&tmp)?;
    let done = file
        .write_all(bytes)
        .and_then(|()| set_mode(&file, mode))
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::rename(&tmp, path));
    if done.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    done
}
