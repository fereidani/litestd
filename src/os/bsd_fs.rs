//! The BSD extensions to primitives in the [`fs`](crate::fs) module, which
//! `os::freebsd::fs`, `os::netbsd::fs`, `os::openbsd::fs` and
//! `os::dragonfly::fs` provide on their systems.

use crate::{
    fs::Metadata,
    os::unix::fs::{forward_stat, stat_methods},
};

/// OS-specific extensions to [`Metadata`]: the fields of the `stat`
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
    /// Returns the creation time of the file, in seconds since the Unix
    /// epoch.
    #[cfg(not(target_os = "dragonfly"))]
    fn st_birthtime(&self) -> i64;
    /// Returns the nanoseconds part of [`st_birthtime`](Self::st_birthtime).
    #[cfg(not(target_os = "dragonfly"))]
    fn st_birthtime_nsec(&self) -> i64;
    /// Returns the preferred block size for efficient file system I/O.
    fn st_blksize(&self) -> u64;
    /// Returns the number of blocks allocated to the file, in 512-byte
    /// units.
    fn st_blocks(&self) -> u64;
    /// Returns the flags set on the file.
    fn st_flags(&self) -> u32;
    /// Returns the file's generation number.
    fn st_gen(&self) -> u32;
    /// Returns a field that the OS reserves. The FreeBSD 12 `stat` that
    /// litestd reads has none, so it is 0 there, where std panics.
    #[cfg(any(target_os = "freebsd", target_os = "dragonfly"))]
    fn st_lspare(&self) -> u32;
}

impl MetadataExt for Metadata {
    stat_methods!();

    forward_stat! {
        #[cfg(not(target_os = "dragonfly"))]
        st_birthtime: i64 = bsd.birthtime.secs;
        #[cfg(not(target_os = "dragonfly"))]
        st_birthtime_nsec: i64 = bsd.birthtime.nanos;
        st_flags: u32 = bsd.flags;
        st_gen: u32 = bsd.generation;
        #[cfg(target_os = "dragonfly")]
        st_lspare: u32 = bsd.lspare;
    }

    #[cfg(target_os = "freebsd")]
    fn st_lspare(&self) -> u32 {
        0
    }
}
