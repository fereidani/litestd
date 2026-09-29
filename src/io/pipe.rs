//! Anonymous pipes: [`pipe`], [`PipeReader`] and [`PipeWriter`].

use alloc_crate::{string::String, vec::Vec};

use super::{IoSlice, IoSliceMut, Read, Result, Write};
use crate::sys::pipe::{self as imp, Pipe};

/// Creates an anonymous pipe, a one-way channel that works across processes.
///
/// A read on the [`PipeReader`] blocks until the pipe holds data, and a write
/// on the [`PipeWriter`] blocks while it is full. Once every copy of the
/// writer is closed, reads return EOF; once every copy of the reader is
/// closed, writes fail with [`BrokenPipe`](super::ErrorKind::BrokenPipe).
/// Both ends can be shared between threads and processes: each byte goes to
/// one reader, and writes above a platform-specific size may interleave.
///
/// On Linux this is `pipe2` with `O_CLOEXEC`; on macOS, as in std, `pipe`
/// and then `FIOCLEX` on each end, which a child that another thread spawns
/// in between inherits; on Windows it is an anonymous pipe with synchronous
/// handles, like those of `CreatePipe`. Child processes inherit neither end
/// unless given one as a `process::Stdio`. The capacity is
/// platform-specific: 64 KiB on Windows and on Linux with 4 KiB pages.
///
/// # Errors
///
/// Fails if the process or the system has no descriptors or handles left.
///
/// # Examples
///
/// ```no_run
/// use litestd::io::{Read, Write, pipe};
///
/// # fn main() -> litestd::io::Result<()> {
/// let (mut reader, mut writer) = pipe()?;
/// writer.write_all(b"hello")?;
/// drop(writer);
/// let mut text = litestd::string::String::new();
/// reader.read_to_string(&mut text)?;
/// assert_eq!(text, "hello");
/// # Ok(())
/// # }
/// ```
#[inline]
pub fn pipe() -> Result<(PipeReader, PipeWriter)> {
    imp::pipe().map(|(reader, writer)| (PipeReader(reader), PipeWriter(writer)))
}

/// Read end of an anonymous pipe, created by [`pipe`]. Reads block, as
/// described there.
#[derive(Debug)]
pub struct PipeReader(pub(crate) Pipe);

/// Write end of an anonymous pipe, created by [`pipe`]. Writes block, as
/// described there.
///
/// On Unix, a write after every reader is gone fails with
/// [`BrokenPipe`](crate::io::ErrorKind::BrokenPipe), as in std: litestd
/// ignores `SIGPIPE` at startup, as std's runtime does.
#[derive(Debug)]
pub struct PipeWriter(pub(crate) Pipe);

impl PipeReader {
    /// Creates a new [`PipeReader`] instance that shares the same underlying
    /// file description.
    ///
    /// # Errors
    ///
    /// Fails if the process has no descriptors or handles left.
    pub fn try_clone(&self) -> Result<Self> {
        self.0.try_clone().map(Self)
    }
}

impl PipeWriter {
    /// Creates a new [`PipeWriter`] instance that shares the same underlying
    /// file description.
    ///
    /// # Errors
    ///
    /// Fails if the process has no descriptors or handles left.
    pub fn try_clone(&self) -> Result<Self> {
        self.0.try_clone().map(Self)
    }
}

/// Implements `Read` for a reader type, reading to the end straight into the
/// spare capacity of the vector.
macro_rules! impl_read {
    ($($t:ty)*) => {$(
        impl Read for $t {
            #[inline]
            fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
                self.0.read(buf)
            }

            #[inline]
            fn read_vectored(
                &mut self,
                bufs: &mut [IoSliceMut<'_>],
            ) -> Result<usize> {
                self.0.read_vectored(bufs)
            }

            #[inline]
            fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
                self.0.read_to_end(buf)
            }

            #[inline]
            fn read_to_string(&mut self, buf: &mut String) -> Result<usize> {
                self.0.read_to_string(buf)
            }
        }
    )*};
}

impl_read! { PipeReader &PipeReader }

/// Implements `Write` for a writer type; there is nothing to flush.
macro_rules! impl_write {
    ($($t:ty)*) => {$(
        impl Write for $t {
            #[inline]
            fn write(&mut self, buf: &[u8]) -> Result<usize> {
                self.0.write(buf)
            }

            #[inline]
            fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> Result<usize> {
                self.0.write_vectored(bufs)
            }

            #[inline]
            fn flush(&mut self) -> Result<()> {
                Ok(())
            }
        }
    )*};
}

impl_write! { PipeWriter &PipeWriter }
