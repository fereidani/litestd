//! Anonymous pipes and [`Pipe`], the owned descriptor that reads and writes
//! them.

use core::{fmt, mem::MaybeUninit};

use alloc_crate::{string::String, vec::Vec};

use super::os;
#[cfg(unix)]
use super::os::cvt;
#[cfg(unix)]
use crate::os::fd::FromRawFd;
use crate::{
    io::{self, IoSlice, IoSliceMut},
    os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd, RawFd},
};

/// An owned descriptor read and written with plain `read` and `write`: an
/// end of a pipe, or any descriptor a child gets as a standard stream.
pub(crate) struct Pipe(OwnedFd);

/// Fails as in std: WASI has no pipes, though a `Pipe` wraps any descriptor.
#[cfg(target_os = "wasi")]
#[allow(clippy::missing_const_for_fn, reason = "stands in for an OS call")]
pub(crate) fn pipe() -> io::Result<(Pipe, Pipe)> {
    Err(crate::sys::unsupported::UNSUPPORTED)
}

/// Creates a pipe whose ends are close-on-exec: `(reader, writer)`. macOS
/// has no `pipe2`: the ends get the flag right after, as in std, so a child
/// that another thread spawns at that moment inherits them.
#[cfg(unix)]
pub(crate) fn pipe() -> io::Result<(Pipe, Pipe)> {
    let mut fds = [0; 2];
    // SAFETY: `fds` is valid for writes of two descriptors.
    #[cfg(not(target_vendor = "apple"))]
    cvt(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) })?;
    // SAFETY: as above.
    #[cfg(target_vendor = "apple")]
    cvt(unsafe { libc::pipe(fds.as_mut_ptr()) })?;
    // SAFETY: the call returned two new descriptors that nothing else owns.
    let ends = fds.map(|fd| Pipe(unsafe { OwnedFd::from_raw_fd(fd) }));
    #[cfg(target_vendor = "apple")]
    for end in &ends {
        os::set_cloexec(end.raw())?;
    }
    Ok(ends.into())
}

impl Pipe {
    pub(crate) fn raw(&self) -> RawFd {
        self.0.as_raw_fd()
    }

    pub(crate) fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }

    pub(crate) fn into_fd(self) -> OwnedFd {
        self.0
    }

    /// Duplicates the descriptor, close-on-exec and numbered 3 or above, so
    /// that it never takes the place of a standard stream.
    pub(crate) fn try_clone(&self) -> io::Result<Self> {
        self.0.try_clone().map(Self)
    }

    pub(crate) fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: `read_uninit` lends the buffer to the kernel alone.
        self.read_uninit(unsafe { io::as_uninit(buf) })
    }

    /// Reads into `buf`, which only the kernel writes: it initializes the
    /// bytes it reports read and touches no others.
    pub(crate) fn read_uninit(
        &self,
        buf: &mut [MaybeUninit<u8>],
    ) -> io::Result<usize> {
        os::read(self.raw(), buf)
    }

    pub(crate) fn read_vectored(
        &self,
        bufs: &mut [IoSliceMut<'_>],
    ) -> io::Result<usize> {
        os::read_vectored(self.raw(), bufs)
    }

    /// Reads to the end straight into the spare capacity of `buf`, which
    /// is never zeroed first.
    pub(crate) fn read_to_end(&self, buf: &mut Vec<u8>) -> io::Result<usize> {
        // SAFETY: `read_uninit` initializes what it reports, nothing else.
        unsafe {
            io::read_to_end_uninit(buf, None, |spare| self.read_uninit(spare))
        }
    }

    pub(crate) fn read_to_string(&self, buf: &mut String) -> io::Result<usize> {
        // SAFETY: `read_to_end` only appends to the vector.
        unsafe { io::append_to_string(buf, |bytes| self.read_to_end(bytes)) }
    }

    pub(crate) fn write(&self, buf: &[u8]) -> io::Result<usize> {
        os::write(self.raw(), buf)
    }

    pub(crate) fn write_vectored(
        &self,
        bufs: &[IoSlice<'_>],
    ) -> io::Result<usize> {
        os::write_vectored(self.raw(), bufs)
    }
}

impl From<OwnedFd> for Pipe {
    fn from(fd: OwnedFd) -> Self {
        Self(fd)
    }
}

impl From<Pipe> for OwnedFd {
    fn from(pipe: Pipe) -> Self {
        pipe.0
    }
}

/// Formats a descriptor as std shows its own descriptor type:
/// `FileDesc(OwnedFd { fd: 3 })`.
pub(crate) struct FileDesc<'a>(pub(crate) &'a OwnedFd);

impl fmt::Debug for FileDesc<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FileDesc").field(self.0).finish()
    }
}

impl fmt::Debug for Pipe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        FileDesc(&self.0).fmt(f)
    }
}
