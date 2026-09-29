//! The [`Write`] trait.

use core::fmt;

use super::{Error, ErrorKind, IoSlice, Result, const_error};

/// A trait for objects which are byte-oriented sinks, through two required
/// methods, [`write`](Write::write) and [`flush`](Write::flush).
pub trait Write {
    /// Writes a buffer into this writer, returning how many bytes were
    /// written.
    ///
    /// `Ok(n)` must satisfy `n <= buf.len()`; `Ok(0)` typically means that the
    /// writer can no longer accept bytes, or that `buf` is empty. Writing only
    /// part of `buf` is not an error.
    ///
    /// # Errors
    ///
    /// Any I/O error, after which no bytes were written. An
    /// [`ErrorKind::Interrupted`] error is non-fatal.
    fn write(&mut self, buf: &[u8]) -> Result<usize>;

    /// Like [`write`](Write::write), except that it writes from a slice of
    /// buffers, consumed in order as one `write` of their concatenation would.
    /// The default writes the first nonempty buffer.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`write`](Write::write).
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> Result<usize> {
        self.write(first_nonempty(bufs))
    }

    /// Flushes this output stream, ensuring that all intermediately buffered
    /// contents reach their destination.
    ///
    /// # Errors
    ///
    /// Fails if not all bytes could be written due to I/O errors or EOF.
    fn flush(&mut self) -> Result<()>;

    /// Attempts to write an entire buffer into this writer.
    ///
    /// # Errors
    ///
    /// The first error other than [`ErrorKind::Interrupted`], which is
    /// retried, or [`ErrorKind::WriteZero`] if `write` accepts no bytes.
    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        default_write_all(self, buf)
    }

    /// Writes a formatted string into this writer, usually through
    /// [`write!`](core::write); a literal without arguments skips formatting.
    ///
    /// # Errors
    ///
    /// Any I/O error. Unlike std, which panics then, a formatting trait
    /// implementation that fails while the writer does not is an error too.
    #[inline]
    fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> Result<()> {
        match args.as_str() {
            Some(s) => self.write_all(s.as_bytes()),
            None => default_write_fmt(self, args),
        }
    }

    /// Creates a "by reference" adapter for this instance of `Write`.
    fn by_ref(&mut self) -> &mut Self
    where
        Self: Sized,
    {
        self
    }
}

/// Returns the first nonempty buffer of `bufs`, or an empty one.
pub(crate) fn first_nonempty<'a>(bufs: &'a [IoSlice<'_>]) -> &'a [u8] {
    bufs.iter()
        .find(|b| !b.is_empty())
        .map_or(&[][..], |b| &**b)
}

/// The provided [`Write::write_all`].
fn default_write_all<W: Write + ?Sized>(
    writer: &mut W,
    mut buf: &[u8],
) -> Result<()> {
    // Each pass consumes at least one byte, stops on an error, or retries
    // after an interruption, so the loop ends once `buf` is empty.
    while !buf.is_empty() {
        match writer.write(buf) {
            Ok(0) => return Err(Error::WRITE_ALL_EOF),
            // A count beyond the buffer, which breaks `write`'s contract,
            // consumes it.
            Ok(n) => buf = buf.get(n..).unwrap_or_default(),
            Err(e) if e.is_interrupted() => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Formats `args` into `writer`. Only the adapter is generic, so every writer
/// shares the formatting machinery.
fn default_write_fmt<W: Write + ?Sized>(
    writer: &mut W,
    args: fmt::Arguments<'_>,
) -> Result<()> {
    /// Forwards formatted pieces to a writer, keeping the first I/O error.
    struct Adapter<'a, T: ?Sized> {
        inner: &'a mut T,
        error: Result<()>,
    }

    impl<T: Write + ?Sized> fmt::Write for Adapter<'_, T> {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            self.inner.write_all(s.as_bytes()).map_err(|e| {
                self.error = Err(e);
                fmt::Error
            })
        }
    }

    let mut adapter = Adapter {
        inner: writer,
        error: Ok(()),
    };
    match fmt::write(&mut adapter, args) {
        Ok(()) => Ok(()),
        Err(fmt::Error) => adapter.error.and(Err(FORMATTER_ERROR)),
    }
}

/// std's error for a formatting trait implementation that failed on its own.
pub(crate) const FORMATTER_ERROR: Error =
    const_error!(ErrorKind::Uncategorized, "formatter error");
