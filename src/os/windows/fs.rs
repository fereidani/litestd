//! Windows-specific extensions to primitives in the [`fs`] module.

use crate::{
    fs::{self, Metadata, OpenOptions},
    io,
    path::Path,
    sys,
    time::SystemTime,
};

mod private {
    /// Seals the extension traits so that, as in std, methods can be added.
    pub trait Sealed {}

    impl Sealed for crate::fs::FileType {}
    impl Sealed for crate::fs::FileTimes {}
}

/// Windows-specific extensions to [`fs::File`].
pub trait FileExt {
    /// Seeks to a given position and reads a number of bytes, returning how
    /// many were read.
    ///
    /// The offset is from the start of the file, and the cursor moves to the
    /// end of the read. A short read is not an error, and reading past the end
    /// returns 0.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    fn seek_read(&self, buf: &mut [u8], offset: u64) -> io::Result<usize>;

    /// Seeks to a given position and writes a number of bytes, returning how
    /// many were written.
    ///
    /// The offset is from the start of the file, and the cursor moves to the
    /// end of the write. A short write is not an error, and writing past the
    /// end extends the file with zeros.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    fn seek_write(&self, buf: &[u8], offset: u64) -> io::Result<usize>;
}

impl FileExt for fs::File {
    fn seek_read(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        self.inner.read_at(buf, offset)
    }

    fn seek_write(&self, buf: &[u8], offset: u64) -> io::Result<usize> {
        self.inner.write_at(buf, offset)
    }
}

/// Windows-specific extensions to [`fs::OpenOptions`].
pub trait OpenOptionsExt {
    /// Overrides the `dwDesiredAccess` argument to `CreateFileW`, replacing the
    /// `read`, `write` and `append` options.
    fn access_mode(&mut self, access: u32) -> &mut Self;

    /// Overrides the `dwShareMode` argument to `CreateFileW`. The default,
    /// `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`, lets other
    /// processes read, write, delete and rename the file while it is open.
    fn share_mode(&mut self, val: u32) -> &mut Self;

    /// Sets extra flags for the `dwFlagsAndAttributes` argument to
    /// `CreateFileW`, replacing earlier custom flags. They cannot clear the
    /// flags that other options set.
    fn custom_flags(&mut self, flags: u32) -> &mut Self;

    /// Sets the file attributes in the `dwFlagsAndAttributes` argument to
    /// `CreateFileW`. A newly created file gets them, and an existing file
    /// opened with `create(true).truncate(true)` gets them added to its own;
    /// otherwise they are ignored.
    fn attributes(&mut self, val: u32) -> &mut Self;

    /// Sets the security quality of service flags for `CreateFileW`, adding
    /// `SECURITY_SQOS_PRESENT`. They limit how far a named pipe's server can
    /// impersonate this process; without them, a privileged process that opens
    /// a user-supplied path can be tricked into lending its privileges.
    fn security_qos_flags(&mut self, flags: u32) -> &mut Self;
}

/// Forwards the methods of `OpenOptionsExt` that take a `u32` to the
/// options of the same name.
macro_rules! forward_options {
    ($($name:ident),*) => {$(
        fn $name(&mut self, value: u32) -> &mut Self {
            self.0.$name(value);
            self
        }
    )*};
}

impl OpenOptionsExt for OpenOptions {
    forward_options!(
        access_mode,
        share_mode,
        custom_flags,
        attributes,
        security_qos_flags
    );
}

/// Windows-specific extensions to [`fs::Metadata`], exposing the fields of
/// `BY_HANDLE_FILE_INFORMATION`.
pub trait MetadataExt {
    /// Returns the value of the `dwFileAttributes` field of this metadata.
    fn file_attributes(&self) -> u32;

    /// Returns the value of the `ftCreationTime` field of this metadata: a
    /// `FILETIME` of 100-nanosecond intervals since 1601-01-01 UTC, or 0 if the
    /// file system does not record it.
    fn creation_time(&self) -> u64;

    /// Returns the value of the `ftLastAccessTime` field of this metadata, in
    /// the unit of [`creation_time`](MetadataExt::creation_time), or 0 if the
    /// file system does not record it.
    fn last_access_time(&self) -> u64;

    /// Returns the value of the `ftLastWriteTime` field of this metadata, in
    /// the unit of [`creation_time`](MetadataExt::creation_time), or 0 if the
    /// file system does not record it.
    fn last_write_time(&self) -> u64;

    /// Returns the value of the `nFileSize` fields of this metadata, which is
    /// meaningless for directories.
    fn file_size(&self) -> u64;
}

impl MetadataExt for Metadata {
    fn file_attributes(&self) -> u32 {
        self.0.file_attributes()
    }

    fn creation_time(&self) -> u64 {
        self.0.creation_time()
    }

    fn last_access_time(&self) -> u64 {
        self.0.last_access_time()
    }

    fn last_write_time(&self) -> u64 {
        self.0.last_write_time()
    }

    fn file_size(&self) -> u64 {
        self.0.size()
    }
}

/// Windows-specific extensions to [`fs::FileType`]. This trait is sealed:
/// it cannot be implemented outside litestd.
pub trait FileTypeExt: private::Sealed {
    /// Returns `true` if this file type is a directory symbolic link.
    fn is_symlink_dir(&self) -> bool;

    /// Returns `true` if this file type is a file symbolic link.
    fn is_symlink_file(&self) -> bool;
}

impl FileTypeExt for fs::FileType {
    fn is_symlink_dir(&self) -> bool {
        self.0.is_symlink_dir()
    }

    fn is_symlink_file(&self) -> bool {
        self.0.is_symlink_file()
    }
}

/// Windows-specific extensions to [`fs::FileTimes`]. This trait is sealed:
/// it cannot be implemented outside litestd.
pub trait FileTimesExt: private::Sealed {
    /// Set the creation time of a file.
    #[allow(
        clippy::return_self_not_must_use,
        reason = "not `#[must_use]` in std"
    )]
    fn set_created(self, t: SystemTime) -> Self;
}

impl FileTimesExt for fs::FileTimes {
    fn set_created(mut self, t: SystemTime) -> Self {
        self.0.set_created(t);
        self
    }
}

/// Creates a file symbolic link `link` pointing to `original`, which should
/// not be a directory; see [`symlink_dir`].
///
/// # Errors
///
/// Fails as the OS reports, with [`io::ErrorKind::PermissionDenied`] if the
/// process lacks `SeCreateSymbolicLinkPrivilege` and Developer Mode is off.
pub fn symlink_file<P: AsRef<Path>, Q: AsRef<Path>>(
    original: P,
    link: Q,
) -> io::Result<()> {
    sys::fs::symlink(original.as_ref(), link.as_ref(), false)
}

/// Creates a directory symbolic link `link` pointing to `original`, which
/// must be a directory; see [`symlink_file`].
///
/// # Errors
///
/// As for [`symlink_file`].
pub fn symlink_dir<P: AsRef<Path>, Q: AsRef<Path>>(
    original: P,
    link: Q,
) -> io::Result<()> {
    sys::fs::symlink(original.as_ref(), link.as_ref(), true)
}
