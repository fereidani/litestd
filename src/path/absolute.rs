//! [`absolute`], which needs the working directory from the OS.

use super::{Path, PathBuf};
use crate::{io, sys};

/// Makes the path absolute without accessing the filesystem.
///
/// Unlike [`canonicalize`], this resolves no symlinks and may succeed for a
/// path that does not exist. On Unix it keeps `..` components and a trailing
/// slash and drops redundant `.` components and separators; on Windows it
/// calls `GetFullPathNameW`, which resolves `..` lexically.
///
/// # Errors
///
/// Fails with [`InvalidInput`] if the path is empty; getting the current
/// directory can fail too.
///
/// [`canonicalize`]: crate::fs::canonicalize
/// [`InvalidInput`]: io::ErrorKind::InvalidInput
pub fn absolute<P: AsRef<Path>>(path: P) -> io::Result<PathBuf> {
    fn inner(path: &Path) -> io::Result<PathBuf> {
        if path.as_os_str().is_empty() {
            return Err(io::const_error!(
                io::ErrorKind::InvalidInput,
                "cannot make an empty path absolute",
            ));
        }
        sys::fs::absolute(path)
    }
    inner(path.as_ref())
}
