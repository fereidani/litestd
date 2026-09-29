//! A file system where the platform has none, as in std: opening, listing
//! and every other operation fails, so no file, directory stream or
//! metadata exists; the builders do, and record nothing.

use core::{convert::Infallible, fmt, mem::MaybeUninit};

use super::UNSUPPORTED;
use crate::{
    ffi::OsString,
    fs::TryLockError,
    io::{self, IoSlice, IoSliceMut, SeekFrom},
    path::{Path, PathBuf},
    time::SystemTime,
};

/// An open file, of which there are none.
pub(crate) struct File(Infallible);

/// The metadata of a file, of which there is none.
#[derive(Clone)]
pub(crate) struct FileAttr(Infallible);

/// A directory stream, of which there are none.
pub(crate) struct ReadDir(Infallible);

/// An entry of a directory, of which there are none.
pub(crate) struct DirEntry(Infallible);

/// Permissions, which only metadata would have.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct FilePermissions(Infallible);

/// A file type, which only metadata would have.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct FileType(Infallible);

/// The times to set on a file.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FileTimes;

/// Options for opening a file.
#[derive(Clone, Debug)]
pub(crate) struct OpenOptions;

/// Options for creating directories.
#[derive(Debug)]
pub(crate) struct DirBuilder;

impl File {
    pub(crate) fn open(_path: &Path, _opts: &OpenOptions) -> io::Result<Self> {
        Err(UNSUPPORTED)
    }
}

unreachable_methods! {
    File;
    fn file_attr(&self) -> io::Result<FileAttr>;
    fn fsync(&self) -> io::Result<()>;
    fn datasync(&self) -> io::Result<()>;
    fn lock(&self) -> io::Result<()>;
    fn lock_shared(&self) -> io::Result<()>;
    fn try_lock(&self) -> Result<(), TryLockError>;
    fn try_lock_shared(&self) -> Result<(), TryLockError>;
    fn unlock(&self) -> io::Result<()>;
    fn truncate(&self, size: u64) -> io::Result<()>;
    fn read(&self, buf: &mut [u8]) -> io::Result<usize>;
    fn read_uninit(&self, buf: &mut [MaybeUninit<u8>]) -> io::Result<usize>;
    fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize>;
    fn write(&self, buf: &[u8]) -> io::Result<usize>;
    fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize>;
    fn flush(&self) -> io::Result<()>;
    fn seek(&self, pos: SeekFrom) -> io::Result<u64>;
    fn tell(&self) -> io::Result<u64>;
    fn duplicate(&self) -> io::Result<Self>;
    fn set_permissions(&self, perm: FilePermissions) -> io::Result<()>;
    fn set_times(&self, times: FileTimes) -> io::Result<()>;
}

impl fmt::Debug for File {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {}
    }
}

unreachable_methods! {
    FileAttr;
    fn size(&self) -> u64;
    fn perm(&self) -> FilePermissions;
    fn file_type(&self) -> FileType;
    fn modified(&self) -> io::Result<SystemTime>;
    fn accessed(&self) -> io::Result<SystemTime>;
    fn created(&self) -> io::Result<SystemTime>;
}

impl FilePermissions {
    pub(crate) fn readonly(self) -> bool {
        match self.0 {}
    }

    pub(crate) fn set_readonly(&mut self, _readonly: bool) {
        match self.0 {}
    }
}

impl FileType {
    pub(crate) fn is_dir(self) -> bool {
        match self.0 {}
    }

    pub(crate) fn is_file(self) -> bool {
        match self.0 {}
    }

    pub(crate) fn is_symlink(self) -> bool {
        match self.0 {}
    }
}

impl FileTimes {
    pub(crate) fn set_accessed(&mut self, _t: SystemTime) {}

    pub(crate) fn set_modified(&mut self, _t: SystemTime) {}
}

impl OpenOptions {
    pub(crate) fn new() -> Self {
        Self
    }

    pub(crate) fn read(&mut self, _read: bool) {}

    pub(crate) fn write(&mut self, _write: bool) {}

    pub(crate) fn append(&mut self, _append: bool) {}

    pub(crate) fn truncate(&mut self, _truncate: bool) {}

    pub(crate) fn create(&mut self, _create: bool) {}

    pub(crate) fn create_new(&mut self, _create_new: bool) {}
}

impl DirBuilder {
    pub(crate) fn new() -> Self {
        Self
    }

    pub(crate) fn mkdir(&self, _path: &Path) -> io::Result<()> {
        Err(UNSUPPORTED)
    }
}

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.0 {}
    }
}

impl fmt::Debug for ReadDir {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {}
    }
}

unreachable_methods! {
    DirEntry;
    fn path(&self) -> PathBuf;
    fn file_name(&self) -> OsString;
    fn metadata(&self) -> io::Result<FileAttr>;
    fn file_type(&self) -> io::Result<FileType>;
}

pub(crate) fn readdir(_path: &Path) -> io::Result<ReadDir> {
    Err(UNSUPPORTED)
}

pub(crate) fn unlink(_path: &Path) -> io::Result<()> {
    Err(UNSUPPORTED)
}

pub(crate) fn rmdir(_path: &Path) -> io::Result<()> {
    Err(UNSUPPORTED)
}

pub(crate) fn remove_dir_all(_path: &Path) -> io::Result<()> {
    Err(UNSUPPORTED)
}

pub(crate) fn rename(_from: &Path, _to: &Path) -> io::Result<()> {
    Err(UNSUPPORTED)
}

pub(crate) fn link(_original: &Path, _link: &Path) -> io::Result<()> {
    Err(UNSUPPORTED)
}

pub(crate) fn stat(_path: &Path) -> io::Result<FileAttr> {
    Err(UNSUPPORTED)
}

pub(crate) fn lstat(_path: &Path) -> io::Result<FileAttr> {
    Err(UNSUPPORTED)
}

pub(crate) fn exists(_path: &Path) -> io::Result<bool> {
    Err(UNSUPPORTED)
}

pub(crate) fn readlink(_path: &Path) -> io::Result<PathBuf> {
    Err(UNSUPPORTED)
}

pub(crate) fn canonicalize(_path: &Path) -> io::Result<PathBuf> {
    Err(UNSUPPORTED)
}

pub(crate) fn copy(_from: &Path, _to: &Path) -> io::Result<u64> {
    Err(UNSUPPORTED)
}

pub(crate) fn set_perm(_path: &Path, perm: FilePermissions) -> io::Result<()> {
    match perm.0 {}
}

pub(crate) fn set_times(_path: &Path, _times: FileTimes) -> io::Result<()> {
    Err(UNSUPPORTED)
}

pub(crate) fn set_times_nofollow(
    _path: &Path,
    _times: FileTimes,
) -> io::Result<()> {
    Err(UNSUPPORTED)
}

/// Fails: no path is absolute without a file system, and std's fallback,
/// the working directory, does not exist.
pub(crate) fn absolute(_path: &Path) -> io::Result<PathBuf> {
    Err(UNSUPPORTED)
}
