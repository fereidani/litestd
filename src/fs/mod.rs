//! Filesystem manipulation operations.
//!
//! Platform-specific functionality lives in the extension traits of
//! `litestd::os::$platform`. Paths shorter than 384 bytes reach the OS
//! without a heap allocation.
//!
//! # Differences from std
//!
//! [`OpenOptions::open`] reports invalid option combinations with a static
//! message and no allocation: [`kind`] and [`Display`] match std's, but
//! [`get_ref`] returns `None`. A directory entry whose name is not a single
//! path component, which only a hostile file system reports and which
//! kernels such as Linux before 5.4 pass on, makes [`ReadDir`] end with
//! [`io::ErrorKind::InvalidData`] and [`remove_dir_all`] fail rather than
//! follow it out of the tree.
//!
//! [`kind`]: io::Error::kind
//! [`Display`]: core::fmt::Display
//! [`get_ref`]: io::Error::get_ref

// std declares these functions without `const`, and so does litestd.
#![allow(clippy::missing_const_for_fn)]

mod dir;
mod file;
mod metadata;
mod path_ext;

use alloc_crate::{string::String, vec::Vec};

pub use self::{
    dir::{DirBuilder, DirEntry, ReadDir},
    file::{File, OpenOptions, TryLockError},
    metadata::{FileTimes, FileType, Metadata, Permissions},
};
use crate::{
    io::{self, Write},
    path::{Path, PathBuf},
    sys,
};

/// Reads the entire contents of a file into a bytes vector, allocated once
/// with the size the file's metadata reports.
///
/// # Errors
///
/// Fails as [`File::open`] does, or on a read error other than
/// [`io::ErrorKind::Interrupted`], which is retried.
pub fn read<P: AsRef<Path>>(path: P) -> io::Result<Vec<u8>> {
    fn inner(path: &Path) -> io::Result<Vec<u8>> {
        let file = File::open(path)?;
        // The file was just opened, so its size is what is left to read.
        let size = file
            .inner
            .file_attr()
            .ok()
            .and_then(|attr| usize::try_from(attr.size()).ok());
        let mut bytes = Vec::new();
        // SAFETY: `read_uninit` lends the buffer to the OS alone, which
        // initializes the bytes it reports read and writes nothing else.
        unsafe {
            io::read_to_end_uninit(&mut bytes, size, |spare| {
                file.inner.read_uninit(spare)
            })?
        };
        Ok(bytes)
    }
    inner(path.as_ref())
}

/// Reads the entire contents of a file into a string.
///
/// # Errors
///
/// Fails as [`read`] does, or if the contents are not valid UTF-8.
pub fn read_to_string<P: AsRef<Path>>(path: P) -> io::Result<String> {
    fn inner(path: &Path) -> io::Result<String> {
        let file = File::open(path)?;
        let size = file
            .inner
            .file_attr()
            .ok()
            .and_then(|attr| usize::try_from(attr.size()).ok());
        let mut string = String::new();
        // SAFETY: as in `read`.
        unsafe {
            io::read_to_string_uninit(&mut string, size, |spare| {
                file.inner.read_uninit(spare)
            })?;
        }
        Ok(string)
    }
    inner(path.as_ref())
}

/// Writes a slice as the entire contents of a file, creating it if it does
/// not exist and truncating it if it does.
///
/// # Errors
///
/// As for [`File::create`] and [`write_all`](Write::write_all).
pub fn write<P: AsRef<Path>, C: AsRef<[u8]>>(
    path: P,
    contents: C,
) -> io::Result<()> {
    fn inner(path: &Path, contents: &[u8]) -> io::Result<()> {
        File::create(path)?.write_all(contents)
    }
    inner(path.as_ref(), contents.as_ref())
}

/// Changes the timestamps of the file or directory at `path`, following
/// symbolic links; [`set_times_nofollow`] does not follow them.
///
/// # Errors
///
/// Fails if the user lacks permission, and with
/// [`io::ErrorKind::InvalidInput`] for a time the OS cannot represent.
pub fn set_times<P: AsRef<Path>>(path: P, times: FileTimes) -> io::Result<()> {
    sys::fs::set_times(path.as_ref(), times.0)
}

/// Changes the timestamps of the file or symlink at `path`, without
/// following a symbolic link.
///
/// # Errors
///
/// As for [`set_times`].
pub fn set_times_nofollow<P: AsRef<Path>>(
    path: P,
    times: FileTimes,
) -> io::Result<()> {
    sys::fs::set_times_nofollow(path.as_ref(), times.0)
}

/// Removes a file from the filesystem.
///
/// # Errors
///
/// Fails if `path` is a directory or does not exist, or if the user lacks
/// permission to remove it.
pub fn remove_file<P: AsRef<Path>>(path: P) -> io::Result<()> {
    sys::fs::unlink(path.as_ref())
}

/// Queries the metadata of `path`, following symbolic links. On Linux this
/// is one `statx` call, and on macOS one `stat`.
///
/// # Errors
///
/// Fails if `path` does not exist or the user lacks permission to query it.
pub fn metadata<P: AsRef<Path>>(path: P) -> io::Result<Metadata> {
    sys::fs::stat(path.as_ref()).map(Metadata)
}

/// Queries the metadata of `path` without following symbolic links.
///
/// # Errors
///
/// As for [`metadata()`].
pub fn symlink_metadata<P: AsRef<Path>>(path: P) -> io::Result<Metadata> {
    sys::fs::lstat(path.as_ref()).map(Metadata)
}

/// Renames a file or directory, replacing `to` if it already exists.
///
/// # Errors
///
/// Fails if `from` does not exist, the user lacks permission, or `from` and
/// `to` are on different filesystems.
pub fn rename<P: AsRef<Path>, Q: AsRef<Path>>(
    from: P,
    to: Q,
) -> io::Result<()> {
    sys::fs::rename(from.as_ref(), to.as_ref())
}

/// Copies the contents and permission bits of `from` over `to`, returning
/// the number of bytes copied.
///
/// On Linux the data moves inside the kernel with `copy_file_range`, falling
/// back to `sendfile` and then to reading and writing. On macOS, as in std,
/// a `to` that does not exist becomes a clone of `from` when both are on
/// one volume that supports clones; otherwise `fcopyfile` copies the data
/// and, into a regular file, the extended attributes, ACL, times and flags
/// too.
///
/// # Errors
///
/// Fails if `from` is not a regular file or does not exist, if `from` cannot
/// be read or `to` written, or if the parent of `to` does not exist.
pub fn copy<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> io::Result<u64> {
    sys::fs::copy(from.as_ref(), to.as_ref())
}

/// Creates a new hard link `link` pointing to `original`.
///
/// # Errors
///
/// Fails if `original` is not a file or does not exist, or if `link` exists.
pub fn hard_link<P: AsRef<Path>, Q: AsRef<Path>>(
    original: P,
    link: Q,
) -> io::Result<()> {
    sys::fs::link(original.as_ref(), link.as_ref())
}

/// Reads a symbolic link, returning the path it points to.
///
/// # Errors
///
/// Fails if `path` is not a symbolic link or does not exist.
pub fn read_link<P: AsRef<Path>>(path: P) -> io::Result<PathBuf> {
    sys::fs::readlink(path.as_ref())
}

/// Returns the canonical, absolute form of a path with all intermediate
/// components normalized and symbolic links resolved. On Windows the result
/// uses the extended-length `\\?\` syntax.
///
/// # Errors
///
/// Fails if `path` does not exist or a non-final component is not a
/// directory.
pub fn canonicalize<P: AsRef<Path>>(path: P) -> io::Result<PathBuf> {
    sys::fs::canonicalize(path.as_ref())
}

/// Creates a new, empty directory at `path`.
///
/// # Errors
///
/// Fails if `path` exists, if a parent does not exist (see
/// [`create_dir_all`]), or if the user lacks permission.
pub fn create_dir<P: AsRef<Path>>(path: P) -> io::Result<()> {
    DirBuilder::new().create(path.as_ref())
}

/// Recursively creates a directory and all of its missing parents. An empty
/// path succeeds without creating anything.
///
/// # Errors
///
/// Fails as [`create_dir`] does, except for a directory that exists or is
/// created concurrently. Some parents may exist after an error.
pub fn create_dir_all<P: AsRef<Path>>(path: P) -> io::Result<()> {
    DirBuilder::new().recursive(true).create(path.as_ref())
}

/// Removes an empty directory; see [`remove_dir_all`] for a non-empty one.
///
/// # Errors
///
/// Fails if `path` does not exist, is not a directory or is not empty, or if
/// the user lacks permission.
pub fn remove_dir<P: AsRef<Path>>(path: P) -> io::Result<()> {
    sys::fs::rmdir(path.as_ref())
}

/// Removes a directory after removing all its contents. Symbolic links are
/// removed, never followed.
///
/// Every entry is opened and removed relative to its parent's handle without
/// following links (`O_NOFOLLOW` on Unix, the reparse point itself on
/// Windows), so a link that replaces a directory mid-walk cannot redirect the
/// removal outside the tree. The walk keeps its state on the heap. Unlike
/// std, Windows holds at most 16,384 directories open: a deeper tree, which
/// no path can name, fails with [`io::ErrorKind::InvalidFilename`].
///
/// # Errors
///
/// As for [`remove_file`] and [`remove_dir`] on every entry, `path` included;
/// [`io::ErrorKind::NotFound`] means that nothing was removed.
pub fn remove_dir_all<P: AsRef<Path>>(path: P) -> io::Result<()> {
    sys::fs::remove_dir_all(path.as_ref())
}

/// Returns an iterator over the entries within a directory, skipping `.`
/// and `..`, in a platform-dependent order.
///
/// # Errors
///
/// Fails if `path` does not exist or is not a directory, or if the process
/// lacks permission to list it.
pub fn read_dir<P: AsRef<Path>>(path: P) -> io::Result<ReadDir> {
    sys::fs::readdir(path.as_ref()).map(ReadDir)
}

/// Changes the permissions of a file or directory, following symbolic links.
///
/// # Errors
///
/// Fails if `path` does not exist or the user lacks permission.
#[allow(clippy::needless_pass_by_value, reason = "std's signature")]
pub fn set_permissions<P: AsRef<Path>>(
    path: P,
    perm: Permissions,
) -> io::Result<()> {
    sys::fs::set_perm(path.as_ref(), perm.0)
}

/// Returns `Ok(true)` if the path points at an existing entity, following
/// symbolic links. Unlike [`Path::exists`], it fails when existence cannot
/// be verified.
///
/// # Errors
///
/// Returns the error of [`metadata()`] other than [`io::ErrorKind::NotFound`].
#[inline]
pub fn exists<P: AsRef<Path>>(path: P) -> io::Result<bool> {
    sys::fs::exists(path.as_ref())
}
