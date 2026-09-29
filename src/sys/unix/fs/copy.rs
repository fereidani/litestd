//! `copy`: on Linux, the kernel copies of `kernel`, then a read and write
//! loop for what they leave, which is all there is on the BSDs, as in std.

#[cfg(any(target_os = "linux", target_os = "android"))]
mod kernel;

use core::ffi::c_int;

use alloc_crate::vec;

#[cfg(any(target_os = "linux", target_os = "android"))]
use self::kernel::copy_data;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
use super::FileType;
use super::{STACK_BUF, attr::fstat, byte_count, cstr, cvt_r, open_c, to_mode};
use crate::{
    io,
    os::fd::{AsRawFd, RawFd},
    path::Path,
};

pub(crate) fn copy(from: &Path, to: &Path) -> io::Result<u64> {
    let (mut buf_from, mut buf_to) = (STACK_BUF, STACK_BUF);
    let from = cstr(from, &mut buf_from)?;
    let reader = open_c(&from, libc::O_RDONLY, 0)?;
    let attr = fstat(reader.as_raw_fd())?;
    if !attr.file_type().is_file() {
        return Err(io::const_error!(
            io::ErrorKind::InvalidInput,
            "the source path is neither a regular file nor a symlink to a \
             regular file",
        ));
    }
    let mode = to_mode(attr.perm().mode());
    let to = cstr(to, &mut buf_to)?;
    let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC;
    let writer = open_c(&to, flags, mode)?;
    let sink = fstat(writer.as_raw_fd())?.file_type();
    // A new file gets `mode` minus the umask, and an existing one keeps its
    // permissions: set them exactly, but on regular files only, so that
    // copying to a device or a FIFO never changes its permissions. WASI has
    // no permissions, and std copies none there.
    #[cfg(unix)]
    if sink.is_file() {
        // SAFETY: `fchmod` touches no memory.
        cvt_r(|| unsafe { libc::fchmod(writer.as_raw_fd(), mode) })?;
    }
    copy_data(reader.as_raw_fd(), writer.as_raw_fd(), attr.size(), sink)
}

/// Copies from the offset of `reader` to its end into `writer`, through
/// the read and write loop: the BSDs have no kernel copy that std uses.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn copy_data(
    reader: RawFd,
    writer: RawFd,
    _len: u64,
    _sink: FileType,
) -> io::Result<u64> {
    read_write(reader, writer, 0)
}

/// Bytes moved per `read` and `write` in the fallback loop.
const FALLBACK_BUF: usize = 64 * 1024;

/// Copies the rest of `reader` through a buffer, adding to `written`. Only
/// special files and systems without kernel copies get here, so the buffer
/// is zeroed rather than lent to the kernel uninitialized.
pub(super) fn read_write(
    reader: RawFd,
    writer: RawFd,
    mut written: u64,
) -> io::Result<u64> {
    let mut buf = vec![0u8; FALLBACK_BUF];
    // Each pass copies at least one byte or returns, so the loop ends at the
    // end of the source.
    loop {
        let (ptr, len) = (buf.as_mut_ptr(), buf.len());
        // SAFETY: `buf` is valid for writes of `len` bytes.
        let n = cvt_r(|| unsafe { libc::read(reader, ptr.cast(), len) })?;
        let Some(data) = buf.get(..byte_count(n)) else {
            // `read` never reports more than it was given room for.
            return Err(io::Error::from_raw_os_error(libc::EIO));
        };
        if data.is_empty() {
            return Ok(written);
        }
        write_all(writer, data)?;
        written += data.len() as u64;
    }
}

fn write_all(fd: c_int, mut data: &[u8]) -> io::Result<()> {
    // Each pass writes at least one byte or returns.
    while !data.is_empty() {
        // SAFETY: `data` is valid for reads of its length.
        let n = cvt_r(|| unsafe {
            libc::write(fd, data.as_ptr().cast(), data.len())
        })?;
        match data.split_at_checked(byte_count(n)) {
            Some(([], _)) => {
                return Err(io::const_error!(
                    io::ErrorKind::WriteZero,
                    "failed to write whole buffer",
                ));
            }
            Some((_, rest)) => data = rest,
            // `write` never reports more than it was given.
            None => return Err(io::Error::from_raw_os_error(libc::EIO)),
        }
    }
    Ok(())
}
