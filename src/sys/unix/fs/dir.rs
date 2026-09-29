//! Directory streams and `remove_dir_all`, on the entries of `getdents64` on
//! Linux and of `readdir` on macOS.

#[cfg(any(target_os = "linux", target_os = "android"))]
mod getdents;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
mod readdir;

use core::{ffi::CStr, fmt};

use alloc_crate::{boxed::Box, sync::Arc, vec::Vec};

#[cfg(any(target_os = "linux", target_os = "android"))]
use self::getdents::{Handle, Records, open};
#[cfg(not(any(target_os = "linux", target_os = "android")))]
use self::readdir::{Handle, Records, open};
use super::{
    FileAttr, FileType, STACK_BUF,
    attr::{self, stat_at},
    cstr, cvt, open_dir_at,
};
use crate::{
    ffi::{OsStr, OsString},
    io,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
};

/// Entry names that fit in this many bytes, NUL included, are stored inside
/// the `DirEntry`, which keeps it at 48 bytes on 64-bit targets: cheap to
/// move, as each entry is moved several times on its way to the caller.
const INLINE_NAME: usize = 22;

/// The error for an entry that the OS reported with a name that is not a
/// single path component.
const MALFORMED: io::Error =
    io::const_error!(io::ErrorKind::InvalidData, "malformed directory entry",);

/// Whether an entry's name is a single path component: kernels, such as
/// Linux before 5.4, may pass on any name a hostile FUSE server reports, and
/// `O_NOFOLLOW` would still follow symlinks in its earlier components, out
/// of the tree.
fn valid_name(name: &[u8]) -> bool {
    !name.is_empty() && !name.contains(&b'/')
}

/// Drops the NUL that ends `name`.
fn without_nul(name: &[u8]) -> &[u8] {
    name.get(..name.len().saturating_sub(1)).unwrap_or_default()
}

/// The name for system calls, as a C string: it ends with its one NUL.
fn c_name(name_with_nul: &[u8]) -> &CStr {
    CStr::from_bytes_with_nul(name_with_nul).unwrap_or_default()
}

/// Maps a `d_type` to the `S_IF*` file type bits, or `None` if the file
/// system did not say.
const fn entry_format(kind: u8) -> Option<libc::mode_t> {
    Some(match kind {
        libc::DT_REG => libc::S_IFREG,
        libc::DT_DIR => libc::S_IFDIR,
        libc::DT_LNK => libc::S_IFLNK,
        libc::DT_CHR => libc::S_IFCHR,
        libc::DT_BLK => libc::S_IFBLK,
        libc::DT_FIFO => libc::S_IFIFO,
        libc::DT_SOCK => libc::S_IFSOCK,
        _ => return None,
    })
}

/// An open directory, shared by its stream and the entries it returned.
struct Dir {
    handle: Handle,
    /// The path the directory was opened with, which entry paths extend.
    root: PathBuf,
}

/// An iterator over the entries of a directory.
pub(crate) struct ReadDir {
    dir: Arc<Dir>,
    records: Records,
    /// Set after the last entry or the first error.
    done: bool,
}

pub(crate) fn readdir(path: &Path) -> io::Result<ReadDir> {
    let mut buf = STACK_BUF;
    let fd = open_dir_at(libc::AT_FDCWD, &cstr(path, &mut buf)?, 0)?;
    Ok(ReadDir {
        dir: Arc::new(Dir {
            handle: open(fd)?,
            root: path.to_path_buf(),
        }),
        records: Records::new(),
        done: false,
    })
}

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;

    fn next(&mut self) -> Option<io::Result<DirEntry>> {
        if self.done {
            return None;
        }
        // SAFETY: only this iterator reads the directory: it opened it, and
        // its entries take no more than the descriptor.
        match unsafe { self.records.advance(&self.dir.handle) } {
            Ok(true) => Some(Ok(DirEntry::new(&self.dir, &self.records))),
            result => {
                // Like std, the stream ends after its last entry or first
                // error. The records are freed; empty ones allocate nothing.
                self.done = true;
                self.records = Records::empty();
                result.err().map(Err)
            }
        }
    }
}

impl fmt::Debug for ReadDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.dir.root, f)
    }
}

/// The name of an entry, NUL included.
enum Name {
    /// Most names: `bytes[..=len]`, where `bytes[len]` is the NUL.
    Inline { len: u8, bytes: [u8; INLINE_NAME] },
    /// Longer names, NUL included.
    Heap(Box<[u8]>),
}

/// An entry of a directory.
pub(crate) struct DirEntry {
    dir: Arc<Dir>,
    #[cfg_attr(
        target_os = "wasi",
        allow(dead_code, reason = "for `os::unix::fs::DirEntryExt`")
    )]
    ino: u64,
    kind: u8,
    name: Name,
}

impl DirEntry {
    fn new(dir: &Arc<Dir>, records: &Records) -> Self {
        let with_nul = records.name_with_nul();
        let mut bytes = [0; INLINE_NAME];
        let inline = bytes.get_mut(..with_nul.len());
        let name = match (inline, u8::try_from(with_nul.len())) {
            (Some(inline), Ok(len_with_nul)) => {
                inline.copy_from_slice(with_nul);
                let len = len_with_nul.saturating_sub(1);
                Name::Inline { len, bytes }
            }
            _ => Name::Heap(with_nul.into()),
        };
        Self {
            dir: Arc::clone(dir),
            ino: records.ino(),
            kind: records.kind(),
            name,
        }
    }

    /// The name, NUL included.
    fn name_with_nul(&self) -> &[u8] {
        match &self.name {
            // `new` stored `len` bytes and a NUL, so this never falls back
            // to an empty name.
            Name::Inline { len, bytes } => {
                bytes.get(..=usize::from(*len)).unwrap_or_default()
            }
            Name::Heap(name) => name,
        }
    }

    fn file_name_os_str(&self) -> &OsStr {
        OsStr::from_unix_bytes(without_nul(self.name_with_nul()))
    }

    pub(crate) fn path(&self) -> PathBuf {
        self.dir.root.join(self.file_name_os_str())
    }

    pub(crate) fn file_name(&self) -> OsString {
        OsString::from_unix_vec(without_nul(self.name_with_nul()).to_vec())
    }

    /// The entry's own metadata, read relative to the directory's descriptor
    /// so that it holds even if the directory has moved.
    pub(crate) fn metadata(&self) -> io::Result<FileAttr> {
        stat_at(
            self.dir.handle.as_raw_fd(),
            c_name(self.name_with_nul()),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    }

    pub(crate) fn file_type(&self) -> io::Result<FileType> {
        entry_format(self.kind).map_or_else(
            || self.metadata().map(|attr| attr.file_type()),
            |format| Ok(FileType::new(format)),
        )
    }

    #[cfg(unix)]
    pub(crate) const fn ino(&self) -> u64 {
        self.ino
    }
}

/// A directory that `remove_dir_all` is emptying.
struct Level {
    dir: Handle,
    records: Records,
    /// Whether this pass over the directory found an entry.
    #[cfg(target_os = "wasi")]
    found: bool,
}

impl Level {
    const fn new(dir: Handle, records: Records) -> Self {
        Self {
            dir,
            records,
            #[cfg(target_os = "wasi")]
            found: false,
        }
    }

    /// Moves to the next entry. Returns `false` at the end of the directory.
    fn advance(&mut self) -> io::Result<bool> {
        // SAFETY: only these records read the directory, which the level
        // opened.
        let more = unsafe { self.records.advance(&self.dir) }?;
        // WASI's `readdir` may skip entries once others are removed, as std
        // notes there: a pass that found entries, which the walk removed, is
        // followed by another from the start, and the last pass finds none.
        #[cfg(target_os = "wasi")]
        if !more && core::mem::take(&mut self.found) {
            // SAFETY: as above.
            unsafe { self.dir.rewind() };
            // SAFETY: as above.
            let more = unsafe { self.records.advance(&self.dir) }?;
            self.found = more;
            return Ok(more);
        }
        #[cfg(target_os = "wasi")]
        {
            self.found |= more;
        }
        Ok(more)
    }
}

/// Returns whether opening a directory failed because the path names
/// something else: a file, or a symlink, which `O_NOFOLLOW` refuses with
/// `ELOOP` on older kernels.
fn is_not_dir(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(libc::ENOTDIR | libc::ELOOP))
}

/// Returns whether the file was gone: someone else removed it first.
fn is_gone(err: &io::Error) -> bool {
    err.raw_os_error() == Some(libc::ENOENT)
}

/// Turns a missing file into success.
fn ignore_not_found(result: io::Result<libc::c_int>) -> io::Result<()> {
    match result {
        Err(e) if is_gone(&e) => Ok(()),
        result => result.map(drop),
    }
}

/// Removes `path` and everything below it, never following a symlink.
pub(crate) fn remove_dir_all(path: &Path) -> io::Result<()> {
    let mut buf = STACK_BUF;
    let path = cstr(path, &mut buf)?;
    // `O_NOFOLLOW` refuses a symlink at `path`, which is removed itself
    // instead. Anything else that is not a directory is an error, as in std.
    let root = match open_dir_at(libc::AT_FDCWD, &path, libc::O_NOFOLLOW) {
        Ok(fd) => open(fd)?,
        Err(e) if is_not_dir(&e) => {
            let attr = attr::stat(&path, false)?;
            if !attr.file_type().is_symlink() {
                return Err(e);
            }
            // SAFETY: `path` is a valid C string.
            return cvt(unsafe { libc::unlink(path.as_ptr()) }).map(drop);
        }
        Err(e) => return Err(e),
    };
    remove_contents(root)?;
    // SAFETY: `path` is a valid C string.
    ignore_not_found(cvt(unsafe { libc::rmdir(path.as_ptr()) }))
}

/// Removes everything inside the directory `root`, depth first.
///
/// Each directory is opened and each entry removed relative to its parent's
/// descriptor, and `O_NOFOLLOW` keeps a symlink swapped in during the walk
/// from leading outside the tree. Open directories wait on the heap, so no
/// depth of tree can overflow the thread's stack.
fn remove_contents(root: Handle) -> io::Result<()> {
    let mut stack = Vec::new();
    stack.push(Level::new(root, Records::new()));
    // The records of the last directory emptied, reused by the next one.
    let mut spare = None;
    // Each pass removes an entry, descends into a directory, or leaves an
    // emptied or vanished directory, and every directory is read to its end
    // once, or on WASI until a pass finds nothing left. The loop ends when
    // the stack is empty: when `root` is empty.
    while let Some(top) = stack.last_mut() {
        let more = match top.advance() {
            Ok(more) => more,
            // As in std, a subdirectory that vanished meanwhile (`ENOENT`) is
            // skipped, while a vanished root fails the removal.
            Err(e) if is_gone(&e) && stack.len() > 1 => {
                // Records whose refill failed read afresh when reused.
                if let Some(gone) = stack.pop() {
                    spare = Some(gone.records);
                }
                continue;
            }
            Err(e) => return Err(e),
        };
        if !more {
            // `top` is empty: close it, and remove it from its parent, whose
            // current entry it is.
            if let Some(done) = stack.pop() {
                spare = Some(done.records);
            }
            if let Some(parent) = stack.last() {
                let fd = parent.dir.as_raw_fd();
                let name = c_name(parent.records.name_with_nul());
                // SAFETY: `name` is a valid C string.
                let r = unsafe {
                    libc::unlinkat(fd, name.as_ptr(), libc::AT_REMOVEDIR)
                };
                ignore_not_found(cvt(r))?;
            }
            continue;
        }
        let name = c_name(top.records.name_with_nul());
        let (fd, kind) = (top.dir.as_raw_fd(), top.records.kind());
        // Some file systems do not report entry types; try those as
        // directories, since unlinking a directory could orphan its contents.
        if kind == libc::DT_DIR || kind == libc::DT_UNKNOWN {
            match open_dir_at(fd, name, libc::O_NOFOLLOW) {
                Ok(child) => {
                    let records = spare.take().unwrap_or_else(Records::new);
                    stack.push(Level::new(open(child)?, records));
                    continue;
                }
                // Not a directory, or no longer: a file or a symlink, which
                // is removed itself.
                Err(e) if is_not_dir(&e) => {}
                Err(e) if is_gone(&e) => continue,
                Err(e) => return Err(e),
            }
        }
        // SAFETY: `name` is a valid C string.
        ignore_not_found(cvt(unsafe { libc::unlinkat(fd, name.as_ptr(), 0) }))?;
    }
    Ok(())
}
