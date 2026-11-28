//! File access in directories the agent's uid owns. The agent can put a link, or a FIFO, at any
//! name there, at any time: nothing here follows a link in the last path component, and modes are
//! set on open descriptors, never by path. For the functions that take a path, the directories
//! above are the caller's to check; the `_beneath` ones open every directory below the home from
//! its parent's descriptor, so none of those can be a link either.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write as _};
use std::os::fd::{AsFd, AsRawFd as _, BorrowedFd, OwnedFd};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Component, Path, PathBuf};

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
/// How often a tree removal goes over a tree deeper than that.
const REMOVE_MAX_PASSES: usize = 1024;

/// Opens an existing regular file for reading. A link is an error, and so is anything that is not
/// a regular file; a FIFO does not block the open.
pub(crate) fn open_regular(path: &Path) -> io::Result<fs::File> {
    let flags = OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags.bits() as i32)
        .open(path)?;
    regular(file)
}

fn regular(file: fs::File) -> io::Result<fs::File> {
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

/// Replaces `name` in the directory `dir` holds open with `bytes` at `mode`: a new temp file
/// beside it (random name, created exclusively, so nothing planted is ever opened), then a rename
/// over `name`. Both are relative to that descriptor, whatever its path names by now. The temp
/// file is removed when any step fails.
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

/// Removes `rel` below `home` unless it is a directory, from its parent's descriptor: a link
/// is removed itself, and none is followed on the way. A path that is not there is no error.
pub(crate) fn remove_file_beneath(home: &Path, rel: &Path) -> io::Result<()> {
    let (parent, name) = rel
        .parent()
        .zip(rel.file_name())
        .ok_or_else(|| invalid("no file name"))?;
    let dir = match open_dir_beneath(home, parent) {
        Ok(dir) => dir,
        // A folder on the way is missing or is not one: nothing is at `rel`.
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    match rustix::fs::unlinkat(&dir, name, AtFlags::empty()) {
        Ok(()) | Err(Errno::NOENT | Errno::ISDIR) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// The last component of `rel` and the path to its folder.
fn split(rel: &Path) -> io::Result<(&Path, &OsStr)> {
    rel.parent()
        .zip(rel.file_name())
        .ok_or_else(|| invalid("no file name"))
}

/// Nothing is there: the entry, or a folder on the way to it, is missing or is not a folder (a
/// link among them).
fn absent(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

fn kind_of(dir: BorrowedFd<'_>, name: &OsStr) -> io::Result<Option<FileType>> {
    match rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => Ok(Some(FileType::from_raw_mode(stat.st_mode))),
        Err(Errno::NOENT) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Removes a link found at any component of `rel` below `home`: the first component, then the
/// second, and so on. A real directory or a missing component is left alone. The link is
/// unlinked from its parent's descriptor, its target never touched. Best effort: what is left
/// in the way makes the open that follows fail.
pub(crate) fn clear_link_chain(home: &Path, rel: &Path) {
    let Ok(mut dir) = open_dir(home) else { return };
    for c in rel.components() {
        let Component::Normal(name) = c else { return };
        if matches!(kind_of(dir.as_fd(), name), Ok(Some(FileType::Symlink))) {
            let _ = rustix::fs::unlinkat(&dir, name, AtFlags::empty());
        }
        let Ok(child) = open_child_dir(dir.as_fd(), name) else {
            return;
        };
        dir = child;
    }
}

/// The folder `rel` below `home`, made where missing (the home too), after the links on the way
/// are removed. A link planted after that is refused, never followed.
pub(crate) fn ensure_dir_beneath(home: &Path, rel: &Path) -> io::Result<OwnedFd> {
    fs::create_dir_all(home)?;
    clear_link_chain(home, rel);
    create_dir_beneath(home, rel)
}

/// [`ensure_dir_beneath`] that also removes a file (anything but a directory) standing where a
/// folder of `rel` goes.
pub(crate) fn replace_dir_beneath(home: &Path, rel: &Path) -> io::Result<OwnedFd> {
    fs::create_dir_all(home)?;
    let mut dir = open_dir(home)?;
    for c in rel.components() {
        let Component::Normal(name) = c else {
            return Err(invalid("not a relative path"));
        };
        if kind_of(dir.as_fd(), name)?.is_some_and(|k| k != FileType::Directory) {
            rustix::fs::unlinkat(&dir, name, AtFlags::empty())?;
        }
        match rustix::fs::mkdirat(&dir, name, Mode::from_raw_mode(0o777)) {
            Ok(()) | Err(Errno::EXIST) => {}
            Err(e) => return Err(e.into()),
        }
        dir = open_child_dir(dir.as_fd(), name)?;
    }
    Ok(dir)
}

/// [`write_atomic_at`] of the file `rel` below `home`, in a folder reached as by
/// [`ensure_dir_beneath`].
pub(crate) fn write_beneath(home: &Path, rel: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let (parent, name) = split(rel)?;
    let dir = ensure_dir_beneath(home, parent)?;
    write_atomic_at(dir.as_fd(), name, bytes, mode)
}

/// [`open_regular`] of the file `name` in the folder `dir` holds open.
pub(crate) fn open_regular_at(dir: BorrowedFd<'_>, name: &OsStr) -> io::Result<fs::File> {
    let flags =
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC;
    regular(fs::File::from(rustix::fs::openat(
        dir,
        name,
        flags,
        Mode::empty(),
    )?))
}

/// [`open_regular`] of `rel` below `home`: no component of `rel` may be a link.
pub(crate) fn open_regular_beneath(home: &Path, rel: &Path) -> io::Result<fs::File> {
    let (parent, name) = split(rel)?;
    open_regular_at(open_dir_beneath(home, parent)?.as_fd(), name)
}

/// Whether anything, a link included, is at `rel` below `home`, no link followed on the way.
pub(crate) fn exists_beneath(home: &Path, rel: &Path) -> bool {
    split(rel).is_ok_and(|(parent, name)| {
        open_dir_beneath(home, parent)
            .and_then(|dir| kind_of(dir.as_fd(), name))
            .is_ok_and(|kind| kind.is_some())
    })
}

/// What is in the folder `dir` holds open, each with its own type (never a link's target's).
pub(crate) fn entries(dir: BorrowedFd<'_>) -> io::Result<Vec<(OsString, FileType)>> {
    Ok(Dir::read_from(dir)?
        .filter_map(Result::ok)
        .map(|e| {
            let name = OsStr::from_bytes(e.file_name().to_bytes()).to_owned();
            (name, e.file_type())
        })
        .filter(|(name, _)| name != "." && name != "..")
        .filter_map(|(name, kind)| match kind {
            FileType::Unknown => kind_of(dir, &name).ok().flatten().map(|k| (name, k)),
            kind => Some((name, kind)),
        })
        .collect())
}

/// [`entries`] of the folder `rel` below `home`; nothing when it is not there, or when it or a
/// folder above it is a link or cannot be read.
pub(crate) fn list_beneath(home: &Path, rel: &Path) -> Vec<(OsString, FileType)> {
    open_dir_beneath(home, rel)
        .and_then(|dir| entries(dir.as_fd()))
        .unwrap_or_default()
}

/// Every entry below `dir` that is not a directory, relative to it. A tree deeper than
/// [`WALK_MAX_DEPTH`] is an error.
pub(crate) fn list_files(dir: OwnedFd) -> io::Result<Vec<PathBuf>> {
    fn walk(dir: &OwnedFd, rel: &Path, depth: usize, out: &mut Vec<PathBuf>) -> io::Result<()> {
        if depth >= WALK_MAX_DEPTH {
            return Err(invalid("tree too deep"));
        }
        for (name, kind) in entries(dir.as_fd())? {
            if kind == FileType::Directory {
                let child = open_child_dir(dir.as_fd(), &name)?;
                walk(&child, &rel.join(&name), depth + 1, out)?;
            } else {
                out.push(rel.join(&name));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(&dir, Path::new(""), 0, &mut out)?;
    Ok(out)
}

/// Removes every link below `rel` (and `rel` or a folder above it, when one is a link),
/// following none.
pub(crate) fn unlink_tree_links(home: &Path, rel: &Path) {
    fn walk(dir: &OwnedFd, depth: usize) {
        let Ok(list) = entries(dir.as_fd()) else {
            return;
        };
        for (name, kind) in list {
            if kind == FileType::Symlink {
                let _ = rustix::fs::unlinkat(dir, &name, AtFlags::empty());
            } else if kind == FileType::Directory
                && depth < WALK_MAX_DEPTH
                && let Ok(child) = open_child_dir(dir.as_fd(), &name)
            {
                walk(&child, depth + 1);
            }
        }
    }
    clear_link_chain(home, rel);
    if let Ok(dir) = open_dir_beneath(home, rel) {
        walk(&dir, 0);
    }
}

/// Removes every file and link below `rel` that `keep` (paths relative to `home`) does not
/// name, then every folder that is left empty. `rel` itself stays.
pub(crate) fn prune_beneath(home: &Path, rel: &Path, keep: &BTreeSet<PathBuf>) {
    fn walk(dir: &OwnedFd, rel: &Path, keep: &BTreeSet<PathBuf>, depth: usize) {
        let Ok(list) = entries(dir.as_fd()) else {
            return;
        };
        for (name, kind) in list {
            let path = rel.join(&name);
            if kind == FileType::Directory {
                if depth < WALK_MAX_DEPTH
                    && let Ok(child) = open_child_dir(dir.as_fd(), &name)
                {
                    walk(&child, &path, keep, depth + 1);
                    // Only succeeds when empty.
                    let _ = rustix::fs::unlinkat(dir, &name, AtFlags::REMOVEDIR);
                }
            } else if matches!(kind, FileType::RegularFile | FileType::Symlink)
                && !keep.contains(&path)
            {
                let _ = rustix::fs::unlinkat(dir, &name, AtFlags::empty());
            }
        }
    }
    if let Ok(dir) = open_dir_beneath(home, rel) {
        walk(&dir, rel, keep, 0);
    }
}

/// Removes `rel` below `home` with all it holds, from its parent's descriptor: nothing is
/// reached through a link. A path that is not there is no error.
pub(crate) fn remove_tree_beneath(home: &Path, rel: &Path) -> io::Result<()> {
    let (parent, name) = split(rel)?;
    let dir = match open_dir_beneath(home, parent) {
        Ok(dir) => dir,
        Err(e) if absent(&e) => return Ok(()),
        Err(e) => return Err(e),
    };
    // A file or a link goes at once.
    match rustix::fs::unlinkat(&dir, name, AtFlags::empty()) {
        Ok(()) | Err(Errno::NOENT) => return Ok(()),
        Err(Errno::ISDIR) => {}
        Err(e) => return Err(e.into()),
    }
    let top = open_child_dir(dir.as_fd(), name)?;
    // What is too deep to walk is moved up to the top, and the tree is gone over again: a
    // bounded number of times, so that one being grown meanwhile cannot hold the caller.
    for _ in 0..REMOVE_MAX_PASSES {
        if !empty_folder(&top, &top, 0)? {
            break;
        }
    }
    match rustix::fs::unlinkat(&dir, name, AtFlags::REMOVEDIR) {
        Ok(()) | Err(Errno::NOENT) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Removes what `dir` (at `depth` below `top`) holds. True: something was moved up to `top`.
fn empty_folder(top: &OwnedFd, dir: &OwnedFd, depth: usize) -> io::Result<bool> {
    let mut moved = false;
    for (name, kind) in entries(dir.as_fd())? {
        if kind != FileType::Directory {
            rustix::fs::unlinkat(dir, &name, AtFlags::empty())?;
        } else if depth < WALK_MAX_DEPTH {
            let child = open_child_dir(dir.as_fd(), &name)?;
            moved |= empty_folder(top, &child, depth + 1)?;
            // It still holds something only when its content was moved up just now.
            match rustix::fs::unlinkat(dir, &name, AtFlags::REMOVEDIR) {
                Ok(()) => {}
                Err(Errno::NOTEMPTY) if moved => {}
                Err(e) => return Err(e.into()),
            }
        } else {
            let to = Uuid::now_v7().simple().to_string();
            rustix::fs::renameat(dir, &name, top, to.as_str())?;
            moved = true;
        }
    }
    Ok(moved)
}

/// The mode a regular file is to get (`None`: keep it), from the open file's metadata and its
/// name.
pub(crate) type FileMode<'a> = &'a dyn Fn(&fs::Metadata, &OsStr) -> Option<u32>;

/// Sets `dir_mode` on `dir` and on every directory below it and, with `file_mode`, the mode it
/// gives for each regular file. Everything is opened from its parent's descriptor and changed
/// through its own: a link, or anything else, is left alone. Best effort.
pub(crate) fn set_tree_modes(dir: OwnedFd, dir_mode: u32, file_mode: Option<FileMode<'_>>) {
    walk_modes(dir, dir_mode, file_mode, 0);
}

fn walk_modes(dir: OwnedFd, dir_mode: u32, file_mode: Option<FileMode<'_>>, depth: usize) {
    // First the mode: it may be what makes the folder listable.
    let _ = rustix::fs::fchmod(&dir, Mode::from_raw_mode(dir_mode));
    if depth >= WALK_MAX_DEPTH {
        return;
    }
    // Collected first, so that only the descriptors of the path are open while going down.
    let Ok(list) = entries(dir.as_fd()) else {
        return;
    };
    for (name, kind) in list {
        if kind == FileType::Directory {
            if let Ok(child) = open_child_dir(dir.as_fd(), &name) {
                walk_modes(child, dir_mode, file_mode, depth + 1);
            }
        } else if let Some(wanted) = file_mode
            && kind == FileType::RegularFile
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
                && let Some(mode) = wanted(&meta, &name)
            {
                let _ = set_mode(&file, mode);
            }
        }
    }
}
