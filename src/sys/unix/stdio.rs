//! The standard streams: file descriptors 0, 1 and 2.

use super::os;
#[cfg(all(feature = "stdio", feature = "io"))]
use crate::io;

/// A standard stream, identified by its file descriptor.
pub(crate) type Stream = libc::c_int;

/// Standard input.
#[cfg(all(feature = "stdio", feature = "io"))]
pub(crate) const STDIN: Stream = libc::STDIN_FILENO;

/// Standard output.
#[cfg(feature = "stdio")]
pub(crate) const STDOUT: Stream = libc::STDOUT_FILENO;

/// Standard error.
pub(crate) const STDERR: Stream = libc::STDERR_FILENO;

/// Writes once from `buf` to `stream`, returning the number of bytes
/// written or the `errno` value.
pub(crate) fn write(stream: Stream, buf: &[u8]) -> Result<usize, i32> {
    let len = buf.len().min(os::MAX_LEN);
    // SAFETY: `buf` is valid for reads of `len` bytes.
    let written = unsafe { libc::write(stream, buf.as_ptr().cast(), len) };
    usize::try_from(written).map_err(|_| os::errno())
}

/// Converts a code that [`write()`] returned to an I/O error.
#[cfg(all(feature = "stdio", feature = "io"))]
pub(crate) fn write_error(code: i32) -> io::Error {
    io::Error::from_raw_os_error(code)
}

/// Does nothing: only Windows consoles keep state between writes.
#[cfg(feature = "panic-location")]
#[cfg_attr(
    any(test, feature = "test-with-std", feature = "custom-panic-handler"),
    allow(dead_code, reason = "litestd's panic handler is compiled out")
)]
pub(crate) const fn reset(_stream: Stream) {}

/// Whether a signal interrupted [`write()`] before it wrote anything.
pub(crate) const fn is_interrupted(code: i32) -> bool {
    code == libc::EINTR
}

/// Whether the error code means that the stream is closed, which std treats
/// as a stream that discards output and has no input.
pub(crate) const fn is_ebadf(code: i32) -> bool {
    code == libc::EBADF
}

/// Returns an identity of the calling thread that no other live thread
/// shares and that is never 0: the address of its `errno`.
#[cfg(feature = "stdio")]
pub(crate) fn thread_id() -> usize {
    os::errno_location().addr()
}

/// Reads standard input; stateless on Unix.
#[cfg(all(feature = "stdio", feature = "io"))]
pub(crate) struct Stdin;

#[cfg(all(feature = "stdio", feature = "io"))]
impl Stdin {
    pub(crate) const fn new() -> Self {
        Self
    }

    /// Reads once into `buf`. Interruptions are returned, as in std.
    #[allow(
        clippy::unused_self,
        clippy::needless_pass_by_ref_mut,
        reason = "the Windows reader has state"
    )]
    pub(crate) fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let len = buf.len().min(os::MAX_LEN);
        // SAFETY: `buf` is valid for writes of `len` bytes.
        let n = unsafe { libc::read(STDIN, buf.as_mut_ptr().cast(), len) };
        usize::try_from(n).map_err(|_| io::Error::last_os_error())
    }
}

/// Whether `stream` is a terminal.
#[cfg(all(feature = "stdio", feature = "io"))]
pub(crate) fn is_terminal(stream: Stream) -> bool {
    super::terminal::is_terminal(stream)
}
