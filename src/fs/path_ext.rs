//! The methods of [`Path`] that query the file system.

use super::{Metadata, ReadDir};
use crate::{
    fs, io,
    path::{Path, PathBuf},
};

impl Path {
    /// Queries the metadata of this path, following symbolic links.
    ///
    /// # Errors
    ///
    /// As for [`fs::metadata()`].
    #[inline]
    pub fn metadata(&self) -> io::Result<Metadata> {
        fs::metadata(self)
    }

    /// Queries the metadata of this path without following symbolic links.
    ///
    /// # Errors
    ///
    /// As for [`fs::symlink_metadata`].
    #[inline]
    pub fn symlink_metadata(&self) -> io::Result<Metadata> {
        fs::symlink_metadata(self)
    }

    /// Returns the canonical, absolute form of the path with all
    /// intermediate components normalized and symbolic links resolved.
    ///
    /// # Errors
    ///
    /// As for [`fs::canonicalize`].
    #[inline]
    pub fn canonicalize(&self) -> io::Result<PathBuf> {
        fs::canonicalize(self)
    }

    /// Reads a symbolic link, returning the path it points to.
    ///
    /// # Errors
    ///
    /// As for [`fs::read_link`].
    #[inline]
    pub fn read_link(&self) -> io::Result<PathBuf> {
        fs::read_link(self)
    }

    /// Returns an iterator over the entries within a directory.
    ///
    /// # Errors
    ///
    /// As for [`fs::read_dir`].
    #[inline]
    pub fn read_dir(&self) -> io::Result<ReadDir> {
        fs::read_dir(self)
    }

    /// Returns `true` if the path points at an existing entity, following
    /// symbolic links. Any error, such as a denied permission, gives `false`;
    /// [`try_exists`](Self::try_exists) reports it instead.
    #[must_use]
    #[inline]
    pub fn exists(&self) -> bool {
        fs::metadata(self).is_ok()
    }

    /// Returns `Ok(true)` if the path points at an existing entity, following
    /// symbolic links; a broken link gives `Ok(false)`.
    ///
    /// # Errors
    ///
    /// As for [`fs::exists`].
    #[inline]
    pub fn try_exists(&self) -> io::Result<bool> {
        fs::exists(self)
    }

    /// Returns `true` if the path exists and is a regular file, following
    /// symbolic links. Any error gives `false`.
    #[must_use]
    pub fn is_file(&self) -> bool {
        fs::metadata(self).is_ok_and(|m| m.is_file())
    }

    /// Returns `true` if the path exists and is a directory, following
    /// symbolic links. Any error gives `false`.
    #[must_use]
    pub fn is_dir(&self) -> bool {
        fs::metadata(self).is_ok_and(|m| m.is_dir())
    }

    /// Returns `true` if the path is a symbolic link, even a broken one. Any
    /// error gives `false`.
    #[must_use]
    pub fn is_symlink(&self) -> bool {
        fs::symlink_metadata(self).is_ok_and(|m| m.is_symlink())
    }
}
