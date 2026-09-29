//! The kernel copies of Linux: `copy_file_range` between regular files and
//! `sendfile` for other cases, before the read and write loop of `copy`.

use core::{
    ffi::c_long,
    ptr,
    sync::atomic::{AtomicBool, AtomicU8, Ordering},
};

#[cfg(all(target_os = "linux", not(target_env = "gnu")))]
use libc::sendfile as sendfile64;
#[cfg(any(
    all(target_os = "linux", target_env = "gnu"),
    target_os = "android"
))]
use libc::sendfile64;

use super::{
    super::{FileType, byte_count, cvt},
    read_write,
};
use crate::{io, os::fd::RawFd};

/// How the kernel copy of a file ended.
enum Copied {
    /// The source reached its end after this many bytes.
    Done(u64),
    /// The method does not apply; the next one continues after this many
    /// bytes, from the file offsets where this one stopped.
    Fallback(u64),
}

/// Copies from the offset of `reader` to its end into `writer`, a file of
/// type `sink`. `len` is the size of `reader`, zero for the files of `/proc`
/// that do not know their size.
pub(super) fn copy_data(
    reader: RawFd,
    writer: RawFd,
    len: u64,
    sink: FileType,
) -> io::Result<u64> {
    let mut written = 0;
    if len > 0 && sink.is_file() {
        match copy_file_range(reader, writer)? {
            Copied::Done(n) => return Ok(n),
            Copied::Fallback(n) => written = n,
        }
    }
    // Pages spliced into a pipe or a socket stay shared with the page cache,
    // so later writes to the source could change the data in flight.
    if len > 0 && !sink.is(libc::S_IFIFO) && !sink.is(libc::S_IFSOCK) {
        match sendfile(reader, writer)? {
            Copied::Done(n) => return Ok(written + n),
            Copied::Fallback(n) => written += n,
        }
    }
    read_write(reader, writer, written)
}

const UNKNOWN: u8 = 0;
const AVAILABLE: u8 = 1;
const UNAVAILABLE: u8 = 2;

/// Whether `copy_file_range` works on this kernel. Relaxed ordering
/// suffices: the state publishes no other data, and a stale value costs at
/// most one failing call.
static COPY_FILE_RANGE: AtomicU8 = AtomicU8::new(UNKNOWN);

/// Calls `copy_file_range` directly: the C library wrapper needs glibc
/// 2.27.
///
/// # Safety
///
/// The offset pointers must be null or valid for reads and writes of an
/// `i64`.
unsafe fn copy_file_range_raw(
    reader: RawFd,
    off_in: *mut i64,
    writer: RawFd,
    off_out: *mut i64,
    len: usize,
) -> c_long {
    // SAFETY: the caller vouches for the offset pointers.
    unsafe {
        libc::syscall(
            libc::SYS_copy_file_range,
            reader,
            off_in,
            writer,
            off_out,
            len,
            0,
        )
    }
}

fn copy_file_range(reader: RawFd, writer: RawFd) -> io::Result<Copied> {
    let state = COPY_FILE_RANGE.load(Ordering::Relaxed);
    if state == UNAVAILABLE {
        return Ok(Copied::Fallback(0));
    }
    let mut written = 0u64;
    // Each pass copies at least one byte or returns, so the loop ends at the
    // end of the source. Chunks of 1 GiB keep offsets far from overflowing.
    loop {
        // SAFETY: null offsets make the kernel use the file offsets.
        let r = unsafe {
            copy_file_range_raw(
                reader,
                ptr::null_mut(),
                writer,
                ptr::null_mut(),
                1 << 30,
            )
        };
        match cvt(r).map(|n| u64::try_from(n).unwrap_or(0)) {
            // Some file systems, such as overlayfs and `/proc` on older
            // kernels, report nothing to copy from a file that has data.
            Ok(0) if written == 0 => return Ok(Copied::Fallback(0)),
            Ok(0) => return Ok(Copied::Done(written)),
            Ok(n) => {
                written += n;
                if state == UNKNOWN {
                    COPY_FILE_RANGE.store(AVAILABLE, Ordering::Relaxed);
                }
            }
            Err(e) => return copy_file_range_error(e, written, state),
        }
    }
}

/// Decides whether an error from `copy_file_range` ends the copy or hands it
/// to the next method.
#[cold]
fn copy_file_range_error(
    e: io::Error,
    written: u64,
    state: u8,
) -> io::Result<Copied> {
    match e.raw_os_error() {
        // The offsets would overflow; the next method copies the rest.
        Some(libc::EOVERFLOW) => Ok(Copied::Fallback(written)),
        // Before any byte was copied, these mean the call does not apply:
        // `ENOSYS` before Linux 4.5, `EXDEV` across file systems, `EINVAL`
        // for special files, `EPERM` from seccomp or immutable files,
        // `EOPNOTSUPP` on some file systems, `EBADF` for an append writer.
        Some(
            code @ (libc::ENOSYS
            | libc::EXDEV
            | libc::EINVAL
            | libc::EPERM
            | libc::EOPNOTSUPP
            | libc::EBADF),
        ) if written == 0 => {
            if state == UNKNOWN
                && matches!(code, libc::ENOSYS | libc::EPERM | libc::EOPNOTSUPP)
            {
                COPY_FILE_RANGE
                    .store(probe_copy_file_range(), Ordering::Relaxed);
            }
            Ok(Copied::Fallback(0))
        }
        _ => Err(e),
    }
}

/// Tells a missing or forbidden `copy_file_range` apart from one that
/// refused these particular files: with invalid descriptors, a working call
/// fails with `EBADF`.
fn probe_copy_file_range() -> u8 {
    // SAFETY: null offsets are always allowed.
    let r = unsafe {
        copy_file_range_raw(-1, ptr::null_mut(), -1, ptr::null_mut(), 1)
    };
    if r == -1 && crate::sys::os::errno() == libc::EBADF {
        AVAILABLE
    } else {
        UNAVAILABLE
    }
}

/// Set once `sendfile` proved missing or forbidden. Relaxed ordering
/// suffices, as for `COPY_FILE_RANGE`.
static SENDFILE_MISSING: AtomicBool = AtomicBool::new(false);

fn sendfile(reader: RawFd, writer: RawFd) -> io::Result<Copied> {
    if SENDFILE_MISSING.load(Ordering::Relaxed) {
        return Ok(Copied::Fallback(0));
    }
    let mut written = 0u64;
    // Each pass copies at least one byte or returns, so the loop ends at the
    // end of the source; `sendfile` moves at most `0x7fff_f000` per call.
    loop {
        // SAFETY: a null offset makes the kernel use `reader`'s file offset.
        let r =
            unsafe { sendfile64(writer, reader, ptr::null_mut(), 0x7fff_f000) };
        match cvt(r).map(byte_count) {
            Ok(0) => return Ok(Copied::Done(written)),
            Ok(n) => written += n as u64,
            Err(e) => {
                return match e.raw_os_error() {
                    Some(libc::ENOSYS | libc::EPERM) => {
                        SENDFILE_MISSING.store(true, Ordering::Relaxed);
                        Ok(Copied::Fallback(written))
                    }
                    Some(libc::EINVAL | libc::EOVERFLOW) => {
                        Ok(Copied::Fallback(written))
                    }
                    _ => Err(e),
                };
            }
        }
    }
}
