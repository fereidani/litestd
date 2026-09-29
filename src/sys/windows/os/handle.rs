//! Synchronous reads and writes through a handle that may have been opened
//! for overlapped I/O, as a parent may pass its children.
//!
//! Each transfer goes through `NtReadFile` or `NtWriteFile` with a status
//! block in this frame, and a pending one is waited for through the handle.
//! If another transfer signals the handle first and this one stays in
//! flight, the process aborts, as std does: returning would free the buffer
//! and status block that the kernel still uses.

#[cfg(feature = "io")]
use core::mem::MaybeUninit;
use core::ptr;

#[cfg(feature = "io")]
use windows_sys::Wdk::Storage::FileSystem::NtReadFile;
use windows_sys::{
    Wdk::Storage::FileSystem::NtWriteFile,
    Win32::{
        Foundation::{
            HANDLE, RtlNtStatusToDosError, STATUS_END_OF_FILE, STATUS_PENDING,
        },
        System::{
            IO::{IO_STATUS_BLOCK, IO_STATUS_BLOCK_0},
            Threading::{INFINITE, WaitForSingleObject},
        },
    },
};

use super::{abort, os_code};
#[cfg(feature = "io")]
use crate::io;

/// A read into, or a write from, a buffer.
pub(crate) enum Transfer<'a> {
    /// A buffer that only the kernel writes.
    #[cfg(feature = "io")]
    Read(&'a mut [MaybeUninit<u8>]),
    Write(&'a [u8]),
}

/// Clamps a buffer length to what one Windows I/O call takes.
pub(crate) fn io_len(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

/// Reads or writes once through the open `handle`, at `offset` or else at
/// the current position. Returns the byte count, 0 at the end of a file, or
/// the Windows error code as std stores it.
#[cfg_attr(
    not(feature = "io"),
    allow(clippy::needless_pass_by_value, reason = "only writes without `io`")
)]
pub(crate) fn transfer(
    handle: HANDLE,
    transfer: Transfer<'_>,
    offset: Option<u64>,
) -> Result<usize, i32> {
    let mut status_block = IO_STATUS_BLOCK {
        Anonymous: IO_STATUS_BLOCK_0 {
            Status: STATUS_PENDING,
        },
        Information: 0,
    };
    let block = &raw mut status_block;
    // The offset is a `LARGE_INTEGER` with the same bits, as in std.
    #[allow(clippy::cast_possible_wrap)]
    let offset = offset.map(|offset| offset as i64);
    let offset = offset.as_ref().map_or(ptr::null(), ptr::from_ref);
    let (status, len, is_read) = match transfer {
        #[cfg(feature = "io")]
        Transfer::Read(buf) => {
            let len = io_len(buf.len());
            // SAFETY: the handle is open; `buf` (`len` bytes) and the status
            // block stay writable, and `offset` null or a live `i64`, until
            // the transfer ends, which this function waits for below.
            // Without event, routine or key, it is a plain read.
            let status = unsafe {
                NtReadFile(
                    handle,
                    ptr::null_mut(),
                    None,
                    ptr::null(),
                    block,
                    buf.as_mut_ptr().cast(),
                    len,
                    offset,
                    ptr::null(),
                )
            };
            (status, len, true)
        }
        Transfer::Write(buf) => {
            let len = io_len(buf.len());
            // SAFETY: as for the read, with `buf` valid for reads.
            let status = unsafe {
                NtWriteFile(
                    handle,
                    ptr::null_mut(),
                    None,
                    ptr::null(),
                    block,
                    buf.as_ptr().cast(),
                    len,
                    offset,
                    ptr::null(),
                )
            };
            (status, len, false)
        }
    };
    let status = if status == STATUS_PENDING {
        // Only overlapped handles get here; a failed wait leaves the status
        // pending, which aborts below.
        // SAFETY: waiting has no memory-safety preconditions.
        unsafe { WaitForSingleObject(handle, INFINITE) };
        // SAFETY: `Status` is initialized, and the kernel writes it when the
        // transfer ends; the block escaped to it, so it is reloaded.
        unsafe { status_block.Anonymous.Status }
    } else {
        status
    };
    match status {
        STATUS_PENDING => abort(),
        STATUS_END_OF_FILE if is_read => Ok(0),
        // A driver that reports more than the buffer holds would have the
        // caller trust bytes that were never transferred.
        status if status >= 0 => Ok(status_block.Information.min(len as usize)),
        // SAFETY: `RtlNtStatusToDosError` has no preconditions.
        status => Err(os_code(unsafe { RtlNtStatusToDosError(status) })),
    }
}

/// Reads once at the current position into `buf`, which only the kernel
/// writes. A pipe whose writers are gone reads 0, as in std.
#[cfg(feature = "io")]
pub(crate) fn read(
    handle: HANDLE,
    buf: &mut [MaybeUninit<u8>],
) -> io::Result<usize> {
    match transfer(handle, Transfer::Read(buf), None)
        .map_err(io::Error::from_raw_os_error)
    {
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(0),
        result => result,
    }
}

/// Writes once at the current position from `buf`.
#[cfg(feature = "io")]
pub(crate) fn write(handle: HANDLE, buf: &[u8]) -> io::Result<usize> {
    transfer(handle, Transfer::Write(buf), None)
        .map_err(io::Error::from_raw_os_error)
}

/// Defines `read`, `read_vectored` and `write_vectored` for a byte stream
/// from its `read_uninit` and `write`. A vectored transfer uses the first
/// non-empty buffer, as std's default methods do. The caller imports `io`,
/// `IoSlice` and `IoSliceMut`.
#[cfg(feature = "io")]
macro_rules! stream_io {
    () => {
        pub(crate) fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
            // SAFETY: `read_uninit` lends the buffer to the kernel alone.
            self.read_uninit(unsafe { io::as_uninit(buf) })
        }

        pub(crate) fn read_vectored(
            &self,
            bufs: &mut [IoSliceMut<'_>],
        ) -> io::Result<usize> {
            let buf = bufs.iter_mut().find(|b| !b.is_empty());
            buf.map_or(Ok(0), |buf| self.read(buf))
        }

        pub(crate) fn write_vectored(
            &self,
            bufs: &[IoSlice<'_>],
        ) -> io::Result<usize> {
            let buf = bufs.iter().find(|b| !b.is_empty());
            buf.map_or(Ok(0), |buf| self.write(buf))
        }
    };
}

/// Defines `read_to_end` and `read_to_string` for a byte stream from its
/// `read_uninit`. The caller imports `io`, `String` and `Vec`.
#[cfg(feature = "io")]
macro_rules! read_to_end_io {
    () => {
        /// Reads to the end straight into the spare capacity of `buf`, which
        /// is never zeroed first.
        pub(crate) fn read_to_end(
            &self,
            buf: &mut Vec<u8>,
        ) -> io::Result<usize> {
            // SAFETY: `read_uninit` initializes the bytes it reports read, and
            // writes nothing else.
            unsafe {
                io::read_to_end_uninit(buf, None, |spare| {
                    self.read_uninit(spare)
                })
            }
        }

        pub(crate) fn read_to_string(
            &self,
            buf: &mut String,
        ) -> io::Result<usize> {
            // SAFETY: `read_to_end` only appends to the vector.
            unsafe {
                io::append_to_string(buf, |bytes| self.read_to_end(bytes))
            }
        }
    };
}

#[cfg(feature = "io")]
pub(crate) use {read_to_end_io, stream_io};
