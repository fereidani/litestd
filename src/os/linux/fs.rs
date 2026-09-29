//! Linux-specific extensions to primitives in the [`fs`](crate::fs) module.

use crate::{fs::Metadata, os::unix::fs::stat_methods};

/// OS-specific extensions to [`fs::Metadata`](Metadata): the fields of the
/// `stat` structure.
///
/// [`os::unix::fs::MetadataExt`](crate::os::unix::fs::MetadataExt) returns
/// the same values under names shared by every Unix.
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
    /// Returns the nanoseconds part of [`st_atime`](Self::st_atime).
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
    /// Returns the preferred block size for efficient file system I/O.
    fn st_blksize(&self) -> u64;
    /// Returns the number of blocks allocated to the file, in 512-byte
    /// units.
    fn st_blocks(&self) -> u64;
}

impl MetadataExt for Metadata {
    stat_methods!();
}
