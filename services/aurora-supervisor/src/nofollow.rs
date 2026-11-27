//! File access in directories the agent's uid owns. The agent can put a link, or a FIFO, at any
//! name there, at any time: nothing here follows a link in the last path component, and modes are
//! set on open descriptors, never by path. For the functions that take a path, the directories
//! above are the caller's to check; the `_beneath` ones open every directory below the home from
//! its parent's descriptor, so none of those can be a link either.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write as _};
use std::os::fd::{AsFd, AsRawFd as _, BorrowedFd, OwnedFd};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Component, Path};

use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags};
use rustix::io::Errno;
use uuid::Uuid;

/// A directory that is no link.
const DIR_FLAGS: OFlags = OFlags::DIRECTORY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
/// How far below its root a tree walk goes. It holds one descriptor per level, and nothing
/// Aurora installs is nearly this deep.
const WALK_MAX_DEPTH: usize = 64;

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
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let name = path.file_name().ok_or_else(|| invalid("no file name"))?;
    write_atomic_at(open_dir(parent)?.as_fd(), name, bytes, mode)
}

/// [`write_atomic`] of `name` in the directory `dir` holds open: the temp file and the rename
/// are relative to that descriptor, whatever its path names by now.
pub(crate) fn write_atomic_at(
    dir: BorrowedFd<'_>,
    name: &OsStr,
    bytes: &[u8],
    mode: u32,
) -> io::Result<()> {
    // Not built from the file's name, which may already be as long as a name can be.
    let tmp = format!(".{}.aurora-tmp", Uuid::now_v7().simple());
    // `O_CREAT | O_EXCL` never follows a link.
    let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut file = fs::File::from(rustix::fs::openat(
        dir,
        tmp.as_str(),
        flags,
        Mode::from_raw_mode(0o600),
    )?);
    let done = file
        .write_all(bytes)
        .and_then(|()| set_mode(&file, mode))
        .and_then(|()| file.sync_all())
        .and_then(|()| Ok(rustix::fs::renameat(dir, tmp.as_str(), dir, name)?));
    if done.is_err() {
        let _ = rustix::fs::unlinkat(dir, tmp.as_str(), AtFlags::empty());
    }
    done
}

fn invalid(what: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, what)
}

/// Opens a directory by a path that is trusted (the home itself) or the caller's to check.
fn open_dir(path: &Path) -> io::Result<OwnedFd> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
    Ok(rustix::fs::open(path, flags, Mode::empty())?)
}

/// Opens the directory `name` of `parent`; a link, or anything but a directory, is an error.
fn open_child_dir(parent: BorrowedFd<'_>, name: &OsStr) -> io::Result<OwnedFd> {
    match rustix::fs::openat(parent, name, OFlags::RDONLY | DIR_FLAGS, Mode::empty()) {
        // The agent took its own read permission away. `O_PATH` needs none; the permission
        // comes back through that descriptor's own `/proc` entry (`fchmod` refuses `O_PATH`).
        Err(Errno::ACCESS) => {
            let held = rustix::fs::openat(parent, name, OFlags::PATH | DIR_FLAGS, Mode::empty())?;
            let mode = rustix::fs::fstat(&held)?.st_mode & 0o7777;
            let entry = format!("/proc/self/fd/{}", held.as_raw_fd());
            fs::set_permissions(entry, fs::Permissions::from_mode(mode | 0o700))?;
            Ok(rustix::fs::openat(
                &held,
                ".",
                OFlags::RDONLY | DIR_FLAGS,
                Mode::empty(),
            )?)
        }
        r => Ok(r?),
    }
}

fn descend(home: &Path, rel: &Path, create: bool) -> io::Result<OwnedFd> {
    let mut dir = open_dir(home)?;
    for c in rel.components() {
        let Component::Normal(name) = c else {
            return Err(invalid("not a relative path"));
        };
        if create {
            match rustix::fs::mkdirat(&dir, name, Mode::from_raw_mode(0o777)) {
                Ok(()) | Err(Errno::EXIST) => {}
                Err(e) => return Err(e.into()),
            }
        }
        dir = open_child_dir(dir.as_fd(), name)?;
    }
    Ok(dir)
}

/// Opens the directory `rel` below `home`, each component from its parent's descriptor.
pub(crate) fn open_dir_beneath(home: &Path, rel: &Path) -> io::Result<OwnedFd> {
    descend(home, rel, false)
}

/// [`open_dir_beneath`] that makes the missing directories.
pub(crate) fn create_dir_beneath(home: &Path, rel: &Path) -> io::Result<OwnedFd> {
    descend(home, rel, true)
}

/// Sets `dir_mode` on `dir` and on every directory below it and, with `file_mode`, the mode it
/// gives for each regular file's mode. Everything is opened from its parent's descriptor and
/// changed through its own: a link, or anything else, is left alone. Best effort.
pub(crate) fn set_tree_modes(dir: OwnedFd, dir_mode: u32, file_mode: Option<fn(u32) -> u32>) {
    walk_modes(dir, dir_mode, file_mode, 0);
}

fn walk_modes(dir: OwnedFd, dir_mode: u32, file_mode: Option<fn(u32) -> u32>, depth: usize) {
    // First the mode: it may be what makes the folder listable.
    let _ = rustix::fs::fchmod(&dir, Mode::from_raw_mode(dir_mode));
    if depth >= WALK_MAX_DEPTH {
        return;
    }
    // Collected first, so that only the descriptors of the path are open while going down.
    let Ok(listing) = Dir::read_from(&dir) else {
        return;
    };
    let entries: Vec<(OsString, FileType)> = listing
        .filter_map(Result::ok)
        .map(|e| {
            let name = OsStr::from_bytes(e.file_name().to_bytes()).to_owned();
            (name, e.file_type())
        })
        .filter(|(name, _)| name != "." && name != "..")
        .collect();
    for (name, kind) in entries {
        if matches!(kind, FileType::Directory | FileType::Unknown)
            && let Ok(child) = open_child_dir(dir.as_fd(), &name)
        {
            walk_modes(child, dir_mode, file_mode, depth + 1);
        } else if let Some(wanted) = file_mode
            && matches!(kind, FileType::RegularFile | FileType::Unknown)
        {
            let flags = OFlags::RDONLY
                | OFlags::NOFOLLOW
                | OFlags::NONBLOCK
                | OFlags::NOCTTY
                | OFlags::CLOEXEC;
            let Ok(fd) = rustix::fs::openat(&dir, &name, flags, Mode::empty()) else {
                continue;
            };
            let file = fs::File::from(fd);
            if let Ok(meta) = file.metadata()
                && meta.is_file()
            {
                let _ = set_mode(&file, wanted(meta.permissions().mode()));
            }
        }
    }
}
