//! Darwin-specific extensions to primitives in the [`fs`] module.

use crate::{
    fs::{self, Metadata},
    os::unix::fs::{forward_stat, stat_methods},
    time::SystemTime,
};

mod private {
    /// Seals [`FileTimesExt`](super::FileTimesExt), as in std.
    pub trait Sealed {}

    impl Sealed for crate::fs::FileTimes {}
}

/// OS-specific extensions to [`fs::Metadata`]: the fields of the `stat`
/// structure.
///
/// [`os::unix::fs::MetadataExt`](crate::os::unix::fs::MetadataExt) returns
/// the same values as the fields that every Unix has.
pub trait MetadataExt {
    /// Returns the ID of the device on which this file resides.
    fn st_dev(&self) -> u64;
    /// Returns the inode number.
    fn st_ino(&self) -> u64;
    /// Returns the file type and mode.
    fn st_mode(&self) -> u32;
    /// Returns the number of hard links to the file.
    fn st_nlink(&self) -> u64;
    /// Returns the user ID of the file's owner.
    fn st_uid(&self) -> u32;
    /// Returns the group ID of the file's owner.
    fn st_gid(&self) -> u32;
    /// Returns the ID of the device that this file represents; only
    /// relevant for special files.
    fn st_rdev(&self) -> u64;
    /// Returns the size of the file in bytes; for a symbolic link, the length
    /// of the path it contains.
    fn st_size(&self) -> u64;
    /// Returns the last access time of the file, in seconds since the Unix
    /// epoch.
    fn st_atime(&self) -> i64;
    /// Returns the nanoseconds part of [`st_atime`](Self::st_atime), as the
    /// OS reports them: negative for times before the epoch.
    fn st_atime_nsec(&self) -> i64;
    /// Returns the last modification time of the file, in seconds since the
    /// Unix epoch.
    fn st_mtime(&self) -> i64;
    /// Returns the nanoseconds part of [`st_mtime`](Self::st_mtime).
    fn st_mtime_nsec(&self) -> i64;
    /// Returns the last status change time of the file, in seconds since the
    /// Unix epoch.
    fn st_ctime(&self) -> i64;
    /// Returns the nanoseconds part of [`st_ctime`](Self::st_ctime).
    fn st_ctime_nsec(&self) -> i64;
    /// Returns the creation time of the file, in seconds since the Unix
    /// epoch.
    fn st_birthtime(&self) -> i64;
    /// Returns the nanoseconds part of [`st_birthtime`](Self::st_birthtime).
    fn st_birthtime_nsec(&self) -> i64;
    /// Returns the preferred block size for efficient file system I/O.
    fn st_blksize(&self) -> u64;
    /// Returns the number of blocks allocated to the file, in 512-byte
    /// units.
    fn st_blocks(&self) -> u64;
    /// Returns the flags set on the file, such as `UF_HIDDEN`.
    fn st_flags(&self) -> u32;
    /// Returns the file's generation number.
    fn st_gen(&self) -> u32;
    /// Returns a field that the OS reserves.
    fn st_lspare(&self) -> u32;
    /// Returns two fields that the OS reserves.
    #[cfg(target_os = "macos")]
    fn st_qspare(&self) -> [u64; 2];
}

impl MetadataExt for Metadata {
    stat_methods!();

    forward_stat! {
        st_birthtime: i64 = bsd.birthtime.secs;
        st_birthtime_nsec: i64 = bsd.birthtime.nanos;
        st_flags: u32 = bsd.flags;
        st_gen: u32 = bsd.generation;
        st_lspare: u32 = bsd.lspare;
        #[cfg(target_os = "macos")]
        st_qspare: [u64; 2] = bsd.qspare;
    }
}

/// OS-specific extensions to [`fs::FileTimes`]. This trait is sealed: it
/// cannot be implemented outside litestd.
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
