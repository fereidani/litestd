//! File metadata. Linux reads it with `statx`, for the birth time and 64-bit
//! timestamps on every target, and falls back to `fstatat`; the `stat` of
//! macOS and the BSDs has the birth time, but DragonFly's, and more fields,
//! which the `os::<platform>::fs` extensions report.

#[cfg(any(target_os = "linux", target_os = "android"))]
mod statx;

use core::ffi::{CStr, c_int};

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
use libc::{fstatat as fstatat64, stat as stat64};
#[cfg(all(target_os = "linux", target_env = "gnu"))]
use libc::{fstatat64, stat64};

use super::{FilePermissions, FileType, cvt, to_mode};
use crate::{io, time::SystemTime};

/// Returns the metadata of `path` relative to `dirfd`, as `fstatat` with
/// `flags` would: `AT_SYMLINK_NOFOLLOW` for the link itself.
pub(super) fn stat_at(
    dirfd: c_int,
    path: &CStr,
    flags: c_int,
) -> io::Result<FileAttr> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if let Some(attr) = statx::stat_at(dirfd, path, flags) {
        return attr;
    }
    // SAFETY: `stat64` holds only integers, for which all zeros is valid;
    // zeroing keeps fields a C library leaves alone readable.
    let mut buf: stat64 = unsafe { core::mem::zeroed() };
    // SAFETY: `path` is a C string and `buf` a writable `stat64`.
    cvt(unsafe { fstatat64(dirfd, path.as_ptr(), &raw mut buf, flags) })?;
    Ok(FileAttr::from_stat(&buf))
}

/// Returns the metadata of `path`, or of the symlink there unless `follow`.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) fn stat(path: &CStr, follow: bool) -> io::Result<FileAttr> {
    let flags = if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
    stat_at(libc::AT_FDCWD, path, flags)
}

/// Returns the metadata of the file `fd` refers to: an empty path relative
/// to the descriptor names the file itself.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) fn fstat(fd: c_int) -> io::Result<FileAttr> {
    stat_at(fd, c"", libc::AT_EMPTY_PATH)
}

/// Returns the metadata of `path`, or of the symlink there unless `follow`,
/// with `stat` or `lstat`, as std reads it there.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub(super) fn stat(path: &CStr, follow: bool) -> io::Result<FileAttr> {
    let call = if follow { libc::stat } else { libc::lstat };
    // SAFETY: as in `stat_at`.
    let mut buf: libc::stat = unsafe { core::mem::zeroed() };
    // SAFETY: `path` is a C string and `buf` a writable `stat`.
    cvt(unsafe { call(path.as_ptr(), &raw mut buf) })?;
    Ok(FileAttr::from_stat(&buf))
}

/// Returns the metadata of the file `fd` refers to.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub(super) fn fstat(fd: c_int) -> io::Result<FileAttr> {
    // SAFETY: as in `stat_at`.
    let mut buf: libc::stat = unsafe { core::mem::zeroed() };
    // SAFETY: `buf` is a writable `stat`.
    cvt(unsafe { libc::fstat(fd, &raw mut buf) })?;
    Ok(FileAttr::from_stat(&buf))
}

/// A file timestamp: seconds since the Unix epoch and nanoseconds, as the
/// OS reports them.
#[derive(Clone, Copy)]
pub(crate) struct Timestamp {
    pub(crate) secs: i64,
    pub(crate) nanos: i64,
}

impl Timestamp {
    /// Converts the timestamp as std does, failing for nanoseconds out of
    /// range. macOS reports a time before the epoch as the seconds rounded
    /// toward zero and negative nanoseconds, a tenth of a second before as
    /// 0 and -900,000,000; std turns that into the usual form.
    fn to_system_time(self) -> io::Result<SystemTime> {
        #[cfg(target_vendor = "apple")]
        let (secs, nanos) = if self.secs <= 0
            && self.secs > i64::MIN
            && self.nanos < 0
            && self.nanos > -1_000_000_000
        {
            (self.secs - 1, self.nanos + 1_000_000_000)
        } else {
            (self.secs, self.nanos)
        };
        #[cfg(not(target_vendor = "apple"))]
        let (secs, nanos) = (self.secs, self.nanos);
        u32::try_from(nanos)
            .ok()
            .and_then(|nanos| SystemTime::from_unix(secs, nanos))
            .ok_or(io::const_error!(
                io::ErrorKind::InvalidData,
                "invalid timestamp",
            ))
    }
}

/// Where the birth time of a file stands on Linux.
#[cfg(any(target_os = "linux", target_os = "android"))]
#[derive(Clone, Copy)]
enum Birth {
    Known(Timestamp),
    /// `statx` works, but the file system does not record birth times.
    NotRecorded,
    /// `statx` is unavailable.
    NoStatx,
}

/// The fields of the `stat` of macOS and the BSDs that Linux lacks.
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "wasi"
)))]
#[derive(Clone, Copy)]
pub(crate) struct BsdStat {
    /// The birth time, which DragonFly does not record.
    #[cfg(not(target_os = "dragonfly"))]
    pub(crate) birthtime: Timestamp,
    pub(crate) flags: u32,
    pub(crate) generation: u32,
    #[cfg(any(target_vendor = "apple", target_os = "dragonfly"))]
    pub(crate) lspare: u32,
    #[cfg(target_vendor = "apple")]
    pub(crate) qspare: [u64; 2],
}

/// The metadata of a file. The fields that `os::unix::fs::MetadataExt` and
/// the platform extensions report are read directly.
#[derive(Clone)]
#[cfg_attr(
    target_os = "wasi",
    allow(dead_code, reason = "WASI has no stable metadata extension")
)]
pub(crate) struct FileAttr {
    pub(crate) dev: u64,
    pub(crate) ino: u64,
    pub(crate) nlink: u64,
    pub(crate) rdev: u64,
    pub(crate) size: u64,
    pub(crate) blksize: u64,
    pub(crate) blocks: u64,
    pub(crate) mode: u32,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
    pub(crate) atime: Timestamp,
    pub(crate) mtime: Timestamp,
    pub(crate) ctime: Timestamp,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    birth: Birth,
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "wasi"
    )))]
    pub(crate) bsd: BsdStat,
}

impl FileAttr {
    // `stat64` field types vary by target; these casts widen, or reinterpret
    // sizes, counts and device numbers, which std reads the same way, as
    // unsigned. `deprecated`: libc deprecates `time_t` on musl ahead of its
    // 64-bit switch, while `stat64` still uses it.
    #[allow(
        clippy::cast_lossless,
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        clippy::unnecessary_cast,
        clippy::useless_conversion,
        deprecated
    )]
    fn from_stat(s: &stat64) -> Self {
        let time = |secs: libc::time_t, nanos: libc::c_long| Timestamp {
            secs: i64::from(secs),
            nanos: i64::from(nanos),
        };
        #[cfg(not(any(target_os = "netbsd", target_os = "wasi")))]
        let (atime, mtime, ctime) = (
            time(s.st_atime, s.st_atime_nsec),
            time(s.st_mtime, s.st_mtime_nsec),
            time(s.st_ctime, s.st_ctime_nsec),
        );
        // WASI keeps each time in a `timespec`.
        #[cfg(target_os = "wasi")]
        let (atime, mtime, ctime) = (
            time(s.st_atim.tv_sec, s.st_atim.tv_nsec),
            time(s.st_mtim.tv_sec, s.st_mtim.tv_nsec),
            time(s.st_ctim.tv_sec, s.st_ctim.tv_nsec),
        );
        // NetBSD names the nanoseconds without an underscore.
        #[cfg(target_os = "netbsd")]
        let (atime, mtime, ctime) = (
            time(s.st_atime, s.st_atimensec),
            time(s.st_mtime, s.st_mtimensec),
            time(s.st_ctime, s.st_ctimensec),
        );
        Self {
            dev: s.st_dev as u64,
            ino: s.st_ino as u64,
            nlink: u64::from(s.st_nlink),
            rdev: s.st_rdev as u64,
            size: s.st_size as u64,
            blksize: s.st_blksize as u64,
            blocks: s.st_blocks as u64,
            mode: u32::from(s.st_mode),
            uid: s.st_uid,
            gid: s.st_gid,
            atime,
            mtime,
            ctime,
            #[cfg(any(target_os = "linux", target_os = "android"))]
            birth: Birth::NoStatx,
            #[cfg(not(any(
                target_os = "linux",
                target_os = "android",
                target_os = "wasi"
            )))]
            bsd: BsdStat {
                #[cfg(any(
                    target_vendor = "apple",
                    target_os = "freebsd",
                    target_os = "openbsd"
                ))]
                birthtime: time(s.st_birthtime, s.st_birthtime_nsec),
                #[cfg(target_os = "netbsd")]
                birthtime: time(s.st_birthtime, s.st_birthtimensec),
                flags: s.st_flags,
                // FreeBSD's is 64 bits wide; std keeps the low half too.
                generation: s.st_gen as u32,
                #[cfg(any(target_vendor = "apple", target_os = "dragonfly"))]
                lspare: s.st_lspare as u32,
                #[cfg(target_vendor = "apple")]
                qspare: s.st_qspare.map(|q| q as u64),
            },
        }
    }

    pub(crate) const fn size(&self) -> u64 {
        self.size
    }

    pub(crate) const fn perm(&self) -> FilePermissions {
        FilePermissions::from_mode(self.mode)
    }

    pub(crate) const fn file_type(&self) -> FileType {
        FileType::new(to_mode(self.mode))
    }

    pub(crate) fn modified(&self) -> io::Result<SystemTime> {
        self.mtime.to_system_time()
    }

    pub(crate) fn accessed(&self) -> io::Result<SystemTime> {
        self.atime.to_system_time()
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub(crate) fn created(&self) -> io::Result<SystemTime> {
        match self.birth {
            Birth::Known(t) => t.to_system_time(),
            Birth::NotRecorded => Err(io::const_error!(
                io::ErrorKind::Unsupported,
                "creation time is not available for the filesystem",
            )),
            Birth::NoStatx => Err(io::const_error!(
                io::ErrorKind::Unsupported,
                "creation time is not available on this platform currently",
            )),
        }
    }

    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "dragonfly",
        target_os = "wasi"
    )))]
    pub(crate) fn created(&self) -> io::Result<SystemTime> {
        self.bsd.birthtime.to_system_time()
    }

    /// The change time on WASI, which records no birth time, as std has it.
    #[cfg(target_os = "wasi")]
    pub(crate) fn created(&self) -> io::Result<SystemTime> {
        self.ctime.to_system_time()
    }

    #[cfg(target_os = "dragonfly")]
    #[allow(clippy::unused_self, reason = "std's signature")]
    pub(crate) const fn created(&self) -> io::Result<SystemTime> {
        Err(io::const_error!(
            io::ErrorKind::Unsupported,
            "creation time is not available on this platform currently",
        ))
    }
}
