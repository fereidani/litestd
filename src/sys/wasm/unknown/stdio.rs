//! The standard streams of a module without an OS: output goes nowhere and
//! input is empty, as in std.

#[cfg(all(feature = "stdio", feature = "io"))]
use crate::io;

/// A standard stream, by the number a descriptor would have.
pub(crate) type Stream = u8;

/// Standard input.
#[cfg(all(feature = "stdio", feature = "io"))]
pub(crate) const STDIN: Stream = 0;

/// Standard output.
#[cfg(feature = "stdio")]
pub(crate) const STDOUT: Stream = 1;

/// Standard error.
pub(crate) const STDERR: Stream = 2;

/// Takes every byte, which goes nowhere, as std's stand-in does.
pub(crate) fn write(_stream: Stream, buf: &[u8]) -> Result<usize, i32> {
    Ok(buf.len())
}

/// Converts a code that [`write()`] returned, which it never does.
#[cfg(all(feature = "stdio", feature = "io"))]
pub(crate) fn write_error(code: i32) -> io::Error {
    io::Error::from_raw_os_error(code)
}

/// Does nothing: the streams keep no state.
#[cfg(feature = "panic-location")]
#[cfg_attr(
    any(test, feature = "test-with-std", feature = "custom-panic-handler"),
    allow(dead_code, reason = "litestd's panic handler is compiled out")
)]
pub(crate) fn reset(_stream: Stream) {}

/// No call is interrupted.
pub(crate) fn is_interrupted(_code: i32) -> bool {
    false
}

/// Every stream is there, as a sink or an empty source.
pub(crate) fn is_ebadf(_code: i32) -> bool {
    false
}

/// The one thread's identity, never 0.
#[cfg(all(feature = "stdio", not(target_feature = "atomics")))]
pub(crate) fn thread_id() -> usize {
    1
}

/// Returns an identity of the calling thread that no other live thread
/// shares and that is never 0: the address of a thread-local.
#[cfg(all(feature = "stdio", target_feature = "atomics"))]
pub(crate) fn thread_id() -> usize {
    #[thread_local]
    static ID: u8 = 0;
    (&raw const ID).addr()
}

/// Reads standard input, which is empty.
#[cfg(all(feature = "stdio", feature = "io"))]
pub(crate) struct Stdin;

#[cfg(all(feature = "stdio", feature = "io"))]
impl Stdin {
    pub(crate) const fn new() -> Self {
        Self
    }

    /// Reads nothing, as std does there.
    #[allow(
        clippy::unused_self,
        clippy::needless_pass_by_ref_mut,
        reason = "the Windows reader has state"
    )]
    pub(crate) fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Ok(0)
    }
}

/// No stream is a terminal.
#[cfg(all(feature = "stdio", feature = "io"))]
pub(crate) fn is_terminal(_stream: Stream) -> bool {
    false
}
