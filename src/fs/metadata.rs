//! [`Metadata`], [`Permissions`], [`FileType`] and [`FileTimes`].

use core::fmt;

use crate::{io, sys, time::SystemTime};

/// Metadata information about a file, returned by [`metadata`] and
/// [`symlink_metadata`].
///
/// [`metadata`]: super::metadata()
/// [`symlink_metadata`]: super::symlink_metadata
#[derive(Clone)]
pub struct Metadata(pub(crate) sys::fs::FileAttr);

/// Representation of the various permissions on a file.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Permissions(pub(crate) sys::fs::FilePermissions);

/// A type of file with accessors for each file type, returned by
/// [`Metadata::file_type`].
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct FileType(pub(crate) sys::fs::FileType);

/// Representation of the various timestamps on a file.
#[derive(Copy, Clone, Debug, Default)]
#[must_use = "must be applied to a file via `File::set_times` to have any effect"]
pub struct FileTimes(pub(crate) sys::fs::FileTimes);

impl Metadata {
    /// Returns the file type for this metadata.
    #[must_use]
    pub fn file_type(&self) -> FileType {
        FileType(self.0.file_type())
    }

    /// Returns `true` if this metadata is for a directory.
    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.file_type().is_dir()
    }

    /// Returns `true` if this metadata is for a regular file.
    #[must_use]
    pub fn is_file(&self) -> bool {
        self.file_type().is_file()
    }

    /// Returns `true` if this metadata is for a symbolic link, which only
    /// [`symlink_metadata`](super::symlink_metadata) reports.
    #[must_use]
    pub fn is_symlink(&self) -> bool {
        self.file_type().is_symlink()
    }

    /// Returns the size of the file, in bytes, this metadata is for.
    #[must_use]
    #[allow(clippy::len_without_is_empty, reason = "std has no `is_empty`")]
    pub fn len(&self) -> u64 {
        self.0.size()
    }

    /// Returns the permissions of the file this metadata is for.
    #[must_use]
    pub fn permissions(&self) -> Permissions {
        Permissions(self.0.perm())
    }

    /// Returns the last modification time listed in this metadata.
    ///
    /// # Errors
    ///
    /// Fails on platforms where this field is not available.
    pub fn modified(&self) -> io::Result<SystemTime> {
        self.0.modified()
    }

    /// Returns the last access time listed in this metadata, which many file
    /// systems update lazily or not at all.
    ///
    /// # Errors
    ///
    /// Fails on platforms where this field is not available.
    pub fn accessed(&self) -> io::Result<SystemTime> {
        self.0.accessed()
    }

    /// Returns the creation time listed in this metadata.
    ///
    /// Unlike std, which needs glibc for it, litestd reads it with musl too.
    ///
    /// # Errors
    ///
    /// Fails with [`io::ErrorKind::Unsupported`] on Linux before 4.11 and on
    /// file systems that do not record it.
    pub fn created(&self) -> io::Result<SystemTime> {
        self.0.created()
    }
}

impl fmt::Debug for Metadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("Metadata");
        debug.field("file_type", &self.file_type());
        debug.field("permissions", &self.permissions());
        debug.field("len", &self.len());
        if let Ok(modified) = self.modified() {
            debug.field("modified", &modified);
        }
        if let Ok(accessed) = self.accessed() {
            debug.field("accessed", &accessed);
        }
        if let Ok(created) = self.created() {
            debug.field("created", &created);
        }
        debug.finish_non_exhaustive()
    }
}

impl Permissions {
    /// Returns `true` if these permissions describe a readonly file. On Unix
    /// that means no write bit is set, whoever the current user is.
    #[must_use = "call `set_readonly` to modify the readonly flag"]
    pub fn readonly(&self) -> bool {
        self.0.readonly()
    }

    /// Sets the readonly flag of this value; apply it to a file with
    /// [`set_permissions`]. On Unix this sets or clears the write bits of the
    /// owner, group and others together.
    ///
    /// [`set_permissions`]: super::set_permissions
    pub fn set_readonly(&mut self, readonly: bool) {
        self.0.set_readonly(readonly);
    }
}

impl FileType {
    /// Tests whether this file type represents a directory.
    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.0.is_dir()
    }

    /// Tests whether this file type represents a regular file.
    #[must_use]
    pub fn is_file(&self) -> bool {
        self.0.is_file()
    }

    /// Tests whether this file type represents a symbolic link, which only
    /// [`symlink_metadata`](super::symlink_metadata) reports.
    #[must_use]
    pub fn is_symlink(&self) -> bool {
        self.0.is_symlink()
    }
}

impl fmt::Debug for FileType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileType")
            .field("is_file", &self.is_file())
            .field("is_dir", &self.is_dir())
            .field("is_symlink", &self.is_symlink())
            .finish_non_exhaustive()
    }
}

impl FileTimes {
    /// Creates a new `FileTimes` with no times set, which changes nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the last access time of a file.
    pub fn set_accessed(mut self, t: SystemTime) -> Self {
        self.0.set_accessed(t);
        self
    }

    /// Set the last modified time of a file.
    pub fn set_modified(mut self, t: SystemTime) -> Self {
        self.0.set_modified(t);
        self
    }
}
