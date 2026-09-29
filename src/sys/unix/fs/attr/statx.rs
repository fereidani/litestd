//! `statx`, which reports the birth time and 64-bit timestamps on every
//! Linux target, from Linux 4.11; kernels before it and seccomp filters that
//! deny it leave `stat_at` to `fstatat`.

use core::{
    ffi::{CStr, c_char, c_int, c_uint},
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};

use super::{Birth, FileAttr, Timestamp};
use crate::io;

/// `statx` fields to fill: those of `stat`, and the birth time.
const STATX_MASK: c_uint = STATX_BASIC_STATS | STATX_BTIME;
const STATX_BASIC_STATS: c_uint = 0x07ff;
const STATX_BTIME: c_uint = 0x0800;
/// Asks for what `stat` would report, without forcing a remote file system
/// to synchronize. It is zero, but spells out the intent.
const AT_STATX_SYNC_AS_STAT: c_int = 0;

/// A `statx_timestamp`.
#[repr(C)]
#[derive(Clone, Copy)]
struct StatxTimestamp {
    tv_sec: i64,
    tv_nsec: u32,
    _reserved: i32,
}

/// The kernel's `struct statx`, which is 256 bytes on every target. The
/// libc crate declares it only for some C libraries.
#[repr(C)]
struct Statx {
    stx_mask: u32,
    stx_blksize: u32,
    _stx_attributes: u64,
    stx_nlink: u32,
    stx_uid: u32,
    stx_gid: u32,
    stx_mode: u16,
    _spare0: u16,
    stx_ino: u64,
    stx_size: u64,
    stx_blocks: u64,
    _stx_attributes_mask: u64,
    stx_atime: StatxTimestamp,
    stx_btime: StatxTimestamp,
    stx_ctime: StatxTimestamp,
    stx_mtime: StatxTimestamp,
    stx_rdev_major: u32,
    stx_rdev_minor: u32,
    stx_dev_major: u32,
    stx_dev_minor: u32,
    /// Fields that litestd does not read, and room for new ones.
    _spare: [u64; 14],
}

const _: () = assert!(size_of::<Statx>() == 256);

impl Statx {
    const ZERO_TIME: StatxTimestamp = StatxTimestamp {
        tv_sec: 0,
        tv_nsec: 0,
        _reserved: 0,
    };

    /// All zeros, so that fields the kernel leaves alone read as zero.
    const ZERO: Self = Self {
        stx_mask: 0,
        stx_blksize: 0,
        _stx_attributes: 0,
        stx_nlink: 0,
        stx_uid: 0,
        stx_gid: 0,
        stx_mode: 0,
        _spare0: 0,
        stx_ino: 0,
        stx_size: 0,
        stx_blocks: 0,
        _stx_attributes_mask: 0,
        stx_atime: Self::ZERO_TIME,
        stx_btime: Self::ZERO_TIME,
        stx_ctime: Self::ZERO_TIME,
        stx_mtime: Self::ZERO_TIME,
        stx_rdev_major: 0,
        stx_rdev_minor: 0,
        stx_dev_major: 0,
        stx_dev_minor: 0,
        _spare: [0; 14],
    };
}

/// Calls `statx` directly: the wrapper needs glibc 2.28 or musl 1.2.3.
///
/// # Safety
///
/// `path` must be null or a valid C string, and `buf` null or valid for
/// writes of a `Statx`.
#[cfg(not(miri))]
unsafe fn statx(
    dirfd: c_int,
    path: *const c_char,
    flags: c_int,
    mask: c_uint,
    buf: *mut Statx,
) -> c_int {
    // SAFETY: the caller upholds the contract of the system call.
    let r = unsafe {
        libc::syscall(libc::SYS_statx, dirfd, path, flags, mask, buf)
    };
    if r == 0 { 0 } else { -1 }
}

/// Calls the C library's `statx`: Miri lacks the raw system call.
///
/// # Safety
///
/// `path` must be null or a valid C string, and `buf` null or valid for
/// writes of a `Statx`.
#[cfg(miri)]
unsafe fn statx(
    dirfd: c_int,
    path: *const c_char,
    flags: c_int,
    mask: c_uint,
    buf: *mut Statx,
) -> c_int {
    // SAFETY: the caller upholds the contract, and `Statx` has the layout
    // of `libc::statx`.
    unsafe { libc::statx(dirfd, path, flags, mask, buf.cast()) }
}

/// Set once `statx` proved missing. Relaxed ordering suffices: the flag
/// publishes no other data, and a stale `false` costs one redundant `statx`
/// call.
static STATX_MISSING: AtomicBool = AtomicBool::new(false);

/// Returns the metadata of `path` relative to `dirfd`, as `fstatat` with
/// `flags` would, or `None` if `statx` is unavailable.
pub(super) fn stat_at(
    dirfd: c_int,
    path: &CStr,
    flags: c_int,
) -> Option<io::Result<FileAttr>> {
    if STATX_MISSING.load(Ordering::Relaxed) {
        return None;
    }
    let mut buf = Statx::ZERO;
    let flags = flags | AT_STATX_SYNC_AS_STAT;
    // SAFETY: `path` is a valid C string and `buf` a `Statx`.
    let r =
        unsafe { statx(dirfd, path.as_ptr(), flags, STATX_MASK, &raw mut buf) };
    if r == 0 {
        return Some(Ok(FileAttr::from_statx(&buf)));
    }
    let err = io::Error::last_os_error();
    if statx_missing(&err) {
        None
    } else {
        Some(Err(err))
    }
}

/// Tells whether `err`, from `statx`, means that the system call is
/// unavailable rather than that it failed for the file, and records it.
#[cold]
fn statx_missing(err: &io::Error) -> bool {
    // `ENOSYS` comes from kernels before 4.11 and `EPERM` from seccomp
    // filters, but a file system may report either for a file. A working
    // `statx` fails with `EFAULT` for a null path, which tells them apart.
    if !matches!(err.raw_os_error(), Some(libc::ENOSYS | libc::EPERM)) {
        return false;
    }
    // SAFETY: null pointers are allowed; they make the call fail.
    let r = unsafe { statx(0, ptr::null(), 0, STATX_MASK, ptr::null_mut()) };
    let missing = r != -1 || crate::sys::os::errno() != libc::EFAULT;
    if missing {
        STATX_MISSING.store(true, Ordering::Relaxed);
    }
    missing
}

impl Timestamp {
    fn from_statx(t: StatxTimestamp) -> Self {
        Self {
            secs: t.tv_sec,
            nanos: t.tv_nsec.into(),
        }
    }
}

impl FileAttr {
    fn from_statx(s: &Statx) -> Self {
        Self {
            dev: libc::makedev(s.stx_dev_major, s.stx_dev_minor),
            ino: s.stx_ino,
            nlink: u64::from(s.stx_nlink),
            rdev: libc::makedev(s.stx_rdev_major, s.stx_rdev_minor),
            size: s.stx_size,
            blksize: u64::from(s.stx_blksize),
            blocks: s.stx_blocks,
            mode: u32::from(s.stx_mode),
            uid: s.stx_uid,
            gid: s.stx_gid,
            atime: Timestamp::from_statx(s.stx_atime),
            mtime: Timestamp::from_statx(s.stx_mtime),
            ctime: Timestamp::from_statx(s.stx_ctime),
            birth: if s.stx_mask & STATX_BTIME == 0 {
                Birth::NotRecorded
            } else {
                Birth::Known(Timestamp::from_statx(s.stx_btime))
            },
        }
    }
}
