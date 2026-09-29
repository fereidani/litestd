//! [`ReadDir`], [`DirEntry`] and [`DirBuilder`].

use core::fmt;

use super::{FileType, Metadata};
use crate::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    sys,
};

/// Iterator over the entries in a directory, returned by [`read_dir`]. It
/// ends after yielding an I/O error as an [`Err`].
///
/// [`read_dir`]: super::read_dir
#[derive(Debug)]
pub struct ReadDir(pub(crate) sys::fs::ReadDir);

/// An entry returned by the [`ReadDir`] iterator. On Unix it holds the open
/// directory, and so a file descriptor, even after the `ReadDir` is dropped.
pub struct DirEntry(pub(crate) sys::fs::DirEntry);

/// A builder used to create directories in various manners.
#[derive(Debug)]
pub struct DirBuilder {
    pub(crate) inner: sys::fs::DirBuilder,
    recursive: bool,
}

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;

    fn next(&mut self) -> Option<io::Result<DirEntry>> {
        self.0.next().map(|entry| entry.map(DirEntry))
    }
}

impl DirEntry {
    /// Returns the `read_dir` path joined with this entry's file name.
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.0.path()
    }

    /// Returns the metadata of this entry without following symlinks. On Unix
    /// it is looked up relative to the open directory, so it still describes
    /// this entry if the directory moved.
    ///
    /// # Errors
    ///
    /// Fails if the entry was removed since it was read, and as for
    /// [`fs::symlink_metadata`](super::symlink_metadata).
    pub fn metadata(&self) -> io::Result<Metadata> {
        self.0.metadata().map(Metadata)
    }

    /// Returns the file type of this entry without following symlinks. On Unix
    /// most file systems report it while the directory is read, so this needs
    /// no system call.
    ///
    /// # Errors
    ///
    /// As for [`DirEntry::metadata`], when the type is not known already.
    pub fn file_type(&self) -> io::Result<FileType> {
        self.0.file_type().map(FileType)
    }

    /// Returns the file name of this entry without any leading path
    /// components.
    #[must_use]
    pub fn file_name(&self) -> OsString {
        self.0.file_name()
    }
}

impl fmt::Debug for DirEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("DirEntry").field(&self.path()).finish()
    }
}

#[allow(
    clippy::new_without_default,
    reason = "std has no `Default` for `DirBuilder`"
)]
impl DirBuilder {
    /// Creates a new non-recursive builder with the default permissions.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: sys::fs::DirBuilder::new(),
            recursive: false,
        }
    }

    /// Sets whether missing parents are created too, with the same
    /// permissions. Defaults to `false`.
    pub fn recursive(&mut self, recursive: bool) -> &mut Self {
        self.recursive = recursive;
        self
    }

    /// Creates the directory at `path` with the options of this builder.
    ///
    /// # Errors
    ///
    /// As for [`fs::create_dir`] or, in recursive mode,
    /// [`fs::create_dir_all`].
    ///
    /// [`fs::create_dir`]: super::create_dir
    /// [`fs::create_dir_all`]: super::create_dir_all
    pub fn create<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        self.create_path(path.as_ref())
    }

    fn create_path(&self, path: &Path) -> io::Result<()> {
        if self.recursive {
            self.create_dir_all(path)
        } else {
            self.inner.mkdir(path)
        }
    }

    /// Creates `path` and its missing ancestors, accepting ones created
    /// concurrently. `mkdir` walks up to the first ancestor that exists and
    /// back down without allocating; the common cases take one `mkdir`, plus a
    /// `stat` if `path` exists.
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        // A root or an empty path needs nothing, and the parent of a
        // single-component relative path is empty.
        let creatable =
            |dir: &Path| !dir.as_os_str().is_empty() && dir.parent().is_some();
        if !creatable(path) {
            return Ok(());
        }
        // Up: count the ancestors, `path` included, that do not exist.
        let mut missing = 0;
        for dir in path.ancestors().take_while(|dir| creatable(dir)) {
            match self.inner.mkdir(dir) {
                Ok(()) => break,
                Err(e) if e.kind() == io::ErrorKind::NotFound => missing += 1,
                // Checking `is_dir` only after `AlreadyExists` spares a
                // `stat` for every other error, such as `PermissionDenied`.
                Err(e)
                    if e.kind() == io::ErrorKind::AlreadyExists
                        && dir.is_dir() =>
                {
                    break;
                }
                Err(e) => return Err(e),
            }
        }
        // Down: create them from the outermost. One that another thread created
        // meanwhile is fine if it is a directory.
        while missing > 0 {
            missing -= 1;
            let Some(dir) = path.ancestors().nth(missing) else {
                break;
            };
            match self.inner.mkdir(dir) {
                Err(e)
                    if e.kind() != io::ErrorKind::AlreadyExists
                        || !dir.is_dir() =>
                {
                    return Err(e);
                }
                _ => {}
            }
        }
        Ok(())
    }
}
