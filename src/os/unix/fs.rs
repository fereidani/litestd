//! Unix-specific extensions to primitives in the [`fs`] module.

use crate::{
    fs::{self, OpenOptions, Permissions},
    io,
    os::fd::{AsFd, AsRawFd},
    path::Path,
    sys,
};

/// Unix-specific extensions to [`fs::File`].
pub trait FileExt {
    /// Reads a number of bytes starting from a given offset, without moving
    /// the file cursor, and returns how many it read; a short read is not an
    /// error.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize>;

    /// Reads the exact number of bytes required to fill `buf` from the given
    /// offset, retrying after [`io::ErrorKind::Interrupted`].
    ///
    /// # Errors
    ///
    /// Fails with [`io::ErrorKind::UnexpectedEof`] at the end of the file, or
    /// with the first other read error; `buf` is then unspecified.
    fn read_exact_at(
        &self,
        mut buf: &mut [u8],
        mut offset: u64,
    ) -> io::Result<()> {
        // Each pass fills at least one byte, retries after an interruption,
        // or returns.
        while !buf.is_empty() {
            match self.read_at(buf, offset) {
                Ok(0) => break,
                Ok(n) => {
                    buf = buf.get_mut(n..).unwrap_or_default();
                    offset += n as u64;
                }
                Err(e) if e.is_interrupted() => {}
                Err(e) => return Err(e),
            }
        }
        if buf.is_empty() {
            Ok(())
        } else {
            Err(io::const_error!(
                io::ErrorKind::UnexpectedEof,
                "failed to fill whole buffer",
            ))
        }
    }

    /// Writes a number of bytes starting from a given offset, without moving
    /// the file cursor, and returns how many it wrote; a short write is not an
    /// error. Writing past the end extends the file with zeros.
    ///
    /// On Linux, `pwrite` ignores the offset of a file opened with `O_APPEND`
    /// and appends instead.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize>;

    /// Writes all of `buf` starting from a given offset, retrying after
    /// [`io::ErrorKind::Interrupted`].
    ///
    /// # Errors
    ///
    /// Fails with [`io::ErrorKind::WriteZero`] if a write makes no progress,
    /// or with the first other write error.
    fn write_all_at(&self, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
        // Each pass writes at least one byte, retries after an interruption,
        // or returns.
        while !buf.is_empty() {
            match self.write_at(buf, offset) {
                Ok(0) => {
                    return Err(io::const_error!(
                        io::ErrorKind::WriteZero,
                        "failed to write whole buffer",
                    ));
                }
                Ok(n) => {
                    buf = buf.get(n..).unwrap_or_default();
                    offset += n as u64;
                }
                Err(e) if e.is_interrupted() => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

impl FileExt for fs::File {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        self.inner.read_at(buf, offset)
    }

    fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize> {
        self.inner.write_at(buf, offset)
    }
}

/// Unix-specific extensions to [`fs::Permissions`].
pub trait PermissionsExt {
    /// Returns the mode permission bits of the underlying file: the whole
    /// `st_mode`, file type bits included.
    fn mode(&self) -> u32;

    /// Sets the underlying raw bits for this set of permissions.
    fn set_mode(&mut self, mode: u32);

    /// Creates `Permissions` from the given set of Unix permission bits.
    fn from_mode(mode: u32) -> Self;
}

impl PermissionsExt for Permissions {
    fn mode(&self) -> u32 {
        self.0.mode()
    }

    fn set_mode(&mut self, mode: u32) {
        *self = Self::from_mode(mode);
    }

    fn from_mode(mode: u32) -> Self {
        Self(sys::fs::FilePermissions::from_mode(mode))
    }
}

/// Unix-specific extensions to [`fs::OpenOptions`].
pub trait OpenOptionsExt {
    /// Sets the mode bits that a new file will be created with: `0o666` by
    /// default, masked by the process's `umask`.
    fn mode(&mut self, mode: u32) -> &mut Self;

    /// Passes custom flags to the `flags` argument of `open`, replacing any
    /// set before. They cannot clear the flags the options set, the access
    /// mode bits are masked out, and `O_CLOEXEC` is always set.
    fn custom_flags(&mut self, flags: i32) -> &mut Self;
}

impl OpenOptionsExt for OpenOptions {
    fn mode(&mut self, mode: u32) -> &mut Self {
        self.0.mode(mode);
        self
    }

    fn custom_flags(&mut self, flags: i32) -> &mut Self {
        self.0.custom_flags(flags);
        self
    }
}

/// Unix-specific extensions to [`fs::Metadata`].
pub trait MetadataExt {
    /// Returns the ID of the device containing the file.
    fn dev(&self) -> u64;
    /// Returns the inode number.
    fn ino(&self) -> u64;
    /// Returns the rights applied to this file: the whole `st_mode`, file
    /// type bits included.
    fn mode(&self) -> u32;
    /// Returns the number of hard links pointing to this file.
    fn nlink(&self) -> u64;
    /// Returns the user ID of the owner of this file.
    fn uid(&self) -> u32;
    /// Returns the group ID of the owner of this file.
    fn gid(&self) -> u32;
    /// Returns the device ID of this file (if it is a special one).
    fn rdev(&self) -> u64;
    /// Returns the total size of this file in bytes.
    fn size(&self) -> u64;
    /// Returns the last access time of the file, in seconds since Unix Epoch.
    fn atime(&self) -> i64;
    /// Returns the nanoseconds part of [`atime`](MetadataExt::atime).
    fn atime_nsec(&self) -> i64;
    /// Returns the last modification time of the file, in seconds since
    /// Unix Epoch.
    fn mtime(&self) -> i64;
    /// Returns the nanoseconds part of [`mtime`](MetadataExt::mtime).
    fn mtime_nsec(&self) -> i64;
    /// Returns the last status change time of the file, in seconds since
    /// Unix Epoch.
    fn ctime(&self) -> i64;
    /// Returns the nanoseconds part of [`ctime`](MetadataExt::ctime).
    fn ctime_nsec(&self) -> i64;
    /// Returns the block size for filesystem I/O.
    fn blksize(&self) -> u64;
    /// Returns the number of 512-byte blocks allocated to the file, fewer
    /// than `st_size / 512` for a file with holes.
    fn blocks(&self) -> u64;
}

/// Defines methods that return a field of the `FileAttr` of a
/// [`fs::Metadata`], or of a part of it: `name: type = field.part;`. Each
/// entry may carry attributes.
macro_rules! forward_stat {
    (
        $($(#[$attr:meta])* $name:ident: $ty:ty = $get:ident $(.$part:ident)*;)*
    ) => {$(
        $(#[$attr])*
        fn $name(&self) -> $ty {
            let attr = &self.0;
            attr.$get$(.$part)*
        }
    )*};
}

/// Defines the `st_*` methods of an `os::<platform>::fs::MetadataExt`
/// implementation for the fields that every Unix has, which [`MetadataExt`]
/// returns under shorter names.
#[cfg(not(target_os = "android"))]
macro_rules! stat_methods {
    () => {
        $crate::os::unix::fs::forward_stat! {
            st_dev: u64 = dev;
            st_ino: u64 = ino;
            st_mode: u32 = mode;
            st_nlink: u64 = nlink;
            st_uid: u32 = uid;
            st_gid: u32 = gid;
            st_rdev: u64 = rdev;
            st_size: u64 = size;
            st_atime: i64 = atime.secs;
            st_atime_nsec: i64 = atime.nanos;
            st_mtime: i64 = mtime.secs;
            st_mtime_nsec: i64 = mtime.nanos;
            st_ctime: i64 = ctime.secs;
            st_ctime_nsec: i64 = ctime.nanos;
            st_blksize: u64 = blksize;
            st_blocks: u64 = blocks;
        }
    };
}

#[cfg(not(target_os = "android"))]
pub(crate) use {forward_stat, stat_methods};

impl MetadataExt for fs::Metadata {
    forward_stat! {
        dev: u64 = dev;
        ino: u64 = ino;
        mode: u32 = mode;
        nlink: u64 = nlink;
        uid: u32 = uid;
        gid: u32 = gid;
        rdev: u64 = rdev;
        size: u64 = size;
        atime: i64 = atime.secs;
        atime_nsec: i64 = atime.nanos;
        mtime: i64 = mtime.secs;
        mtime_nsec: i64 = mtime.nanos;
        ctime: i64 = ctime.secs;
        ctime_nsec: i64 = ctime.nanos;
        blksize: u64 = blksize;
        blocks: u64 = blocks;
    }
}

/// Unix-specific extensions for [`fs::FileType`].
pub trait FileTypeExt {
    /// Returns `true` if this file type is a block device.
    fn is_block_device(&self) -> bool;
    /// Returns `true` if this file type is a char device.
    fn is_char_device(&self) -> bool;
    /// Returns `true` if this file type is a fifo.
    fn is_fifo(&self) -> bool;
    /// Returns `true` if this file type is a socket.
    fn is_socket(&self) -> bool;
}

impl FileTypeExt for fs::FileType {
    fn is_block_device(&self) -> bool {
        self.0.is(libc::S_IFBLK)
    }

    fn is_char_device(&self) -> bool {
        self.0.is(libc::S_IFCHR)
    }

    fn is_fifo(&self) -> bool {
        self.0.is(libc::S_IFIFO)
    }

    fn is_socket(&self) -> bool {
        self.0.is(libc::S_IFSOCK)
    }
}

/// Unix-specific extension methods for [`fs::DirEntry`].
pub trait DirEntryExt {
    /// Returns the underlying `d_ino` field in the contained `dirent`
    /// structure.
    fn ino(&self) -> u64;
}

impl DirEntryExt for fs::DirEntry {
    fn ino(&self) -> u64 {
        self.0.ino()
    }
}

/// Creates a new symbolic link `link` pointing to the `original` path.
///
/// # Errors
///
/// Fails if `link` already exists, or with another OS error.
pub fn symlink<P: AsRef<Path>, Q: AsRef<Path>>(
    original: P,
    link: Q,
) -> io::Result<()> {
    sys::fs::symlink(original.as_ref(), link.as_ref())
}

/// Unix-specific extensions to [`fs::DirBuilder`].
pub trait DirBuilderExt {
    /// Sets the mode to create new directories with. This option defaults
    /// to `0o777`; the OS masks out bits with the process's `umask`.
    fn mode(&mut self, mode: u32) -> &mut Self;
}

impl DirBuilderExt for fs::DirBuilder {
    fn mode(&mut self, mode: u32) -> &mut Self {
        self.inner.set_mode(mode);
        self
    }
}

/// Changes the owner and group of the specified path, following symlinks; a
/// `None` uid or gid is left unchanged.
///
/// # Errors
///
/// Returns the error the OS reports, such as `PermissionDenied`.
pub fn chown<P: AsRef<Path>>(
    dir: P,
    uid: Option<u32>,
    gid: Option<u32>,
) -> io::Result<()> {
    sys::fs::chown(
        dir.as_ref(),
        uid.unwrap_or(u32::MAX),
        gid.unwrap_or(u32::MAX),
    )
}

/// Changes the owner and group of the file referenced by the specified open
/// file descriptor, like [`chown`].
///
/// # Errors
///
/// As for [`chown`].
pub fn fchown<F: AsFd>(
    fd: F,
    uid: Option<u32>,
    gid: Option<u32>,
) -> io::Result<()> {
    sys::fs::fchown(
        fd.as_fd().as_raw_fd(),
        uid.unwrap_or(u32::MAX),
        gid.unwrap_or(u32::MAX),
    )
}

/// Changes the owner and group of the specified path, without dereferencing
/// symbolic links; otherwise like [`chown`].
///
/// # Errors
///
/// As for [`chown`].
pub fn lchown<P: AsRef<Path>>(
    dir: P,
    uid: Option<u32>,
    gid: Option<u32>,
) -> io::Result<()> {
    sys::fs::lchown(
        dir.as_ref(),
        uid.unwrap_or(u32::MAX),
        gid.unwrap_or(u32::MAX),
    )
}

/// Changes the root directory of the current process to the specified path,
/// leaving the current directory unchanged.
///
/// # Errors
///
/// Returns the error the OS reports, such as `PermissionDenied`.
pub fn chroot<P: AsRef<Path>>(dir: P) -> io::Result<()> {
    sys::fs::chroot(dir.as_ref())
}
