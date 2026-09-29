//! [`BufWriter`] and [`WriterPanicked`].

use core::{
    error, fmt,
    mem::{self, ManuallyDrop},
    ptr,
};

use alloc_crate::vec::Vec;

use crate::io::{
    DEFAULT_BUF_SIZE, ErrorKind, IntoInnerError, IoSlice, Result, Seek,
    SeekFrom, Write, const_error,
};

/// Wraps a writer and buffers its output.
///
/// It makes small, repeated writes cheap by writing to the inner writer in
/// large, infrequent batches. Dropping it writes out the buffer but ignores
/// any error, so call [`flush`](BufWriter::flush) first.
pub struct BufWriter<W: ?Sized + Write> {
    /// Never grows beyond its initial capacity.
    buf: Vec<u8>,
    inner: W,
}

impl<W: Write> BufWriter<W> {
    /// Creates a new `BufWriter<W>` with a default buffer capacity. The
    /// default is currently 8 KiB, but may change in the future.
    pub fn new(inner: W) -> Self {
        Self::with_capacity(DEFAULT_BUF_SIZE, inner)
    }

    /// Creates a new `BufWriter<W>` with at least the specified buffer
    /// capacity.
    pub fn with_capacity(capacity: usize, inner: W) -> Self {
        Self {
            buf: Vec::with_capacity(capacity),
            inner,
        }
    }

    /// Unwraps this `BufWriter<W>`, returning the underlying writer once the
    /// buffer is written out.
    ///
    /// # Errors
    ///
    /// Fails with the error and the `BufWriter` if flushing the buffer fails.
    pub fn into_inner(
        mut self,
    ) -> core::result::Result<W, IntoInnerError<Self>> {
        match self.flush_buf() {
            Ok(()) => Ok(self.into_parts().0),
            Err(e) => Err(IntoInnerError(self, e)),
        }
    }

    /// Disassembles this `BufWriter<W>`, returning the underlying writer and
    /// the buffered but unwritten data. Panics abort in litestd, so the data
    /// is always `Ok`.
    pub fn into_parts(
        self,
    ) -> (W, core::result::Result<Vec<u8>, WriterPanicked>) {
        let mut this = ManuallyDrop::new(self);
        let buf = mem::take(&mut this.buf);
        // SAFETY: `this` is never dropped or used again, so `inner` is
        // moved out exactly once. What remains of `this`, an empty vector,
        // owns no memory.
        let inner = unsafe { ptr::read(&raw const this.inner) };
        (inner, Ok(buf))
    }
}

impl<W: ?Sized + Write> BufWriter<W> {
    /// Writes out the buffer, removing whatever was written from it even if
    /// an error stops the writing.
    pub(super) fn flush_buf(&mut self) -> Result<()> {
        let mut written = 0;
        // Each pass writes at least one byte, stops on an error, or retries
        // after an interruption, so the loop ends once the buffer is out.
        let result = loop {
            let Some(rest) = self.buf.get(written..).filter(|r| !r.is_empty())
            else {
                break Ok(());
            };
            match self.inner.write(rest) {
                Ok(0) => {
                    break Err(const_error!(
                        ErrorKind::WriteZero,
                        "failed to write the buffered data",
                    ));
                }
                Ok(n) => written += n.min(rest.len()),
                Err(e) if e.is_interrupted() => {}
                Err(e) => break Err(e),
            }
        };
        if written > 0 {
            self.buf.drain(..written.min(self.buf.len()));
        }
        result
    }

    /// Buffers as much of `buf` as fits and returns how much that is.
    pub(super) fn write_to_buf(&mut self, buf: &[u8]) -> usize {
        let n = self.spare_capacity().min(buf.len());
        self.buf.extend_from_slice(buf.get(..n).unwrap_or_default());
        n
    }

    /// Gets a reference to the underlying writer.
    #[allow(clippy::missing_const_for_fn, reason = "not `const` in std")]
    pub fn get_ref(&self) -> &W {
        &self.inner
    }

    /// Gets a mutable reference to the underlying writer.
    #[allow(clippy::missing_const_for_fn, reason = "not `const` in std")]
    pub fn get_mut(&mut self) -> &mut W {
        &mut self.inner
    }

    /// Returns a reference to the internally buffered data.
    pub fn buffer(&self) -> &[u8] {
        &self.buf
    }

    /// Returns the number of bytes the internal buffer can hold without
    /// flushing.
    pub fn capacity(&self) -> usize {
        self.buf.capacity()
    }

    #[inline]
    fn spare_capacity(&self) -> usize {
        self.buf.capacity() - self.buf.len()
    }

    /// The slow path of `write`: flushes first if `buf` does not fit, and
    /// writes `buf` directly if it would fill the buffer on its own.
    #[cold]
    #[inline(never)]
    fn write_cold(&mut self, buf: &[u8]) -> Result<usize> {
        if buf.len() > self.spare_capacity() {
            self.flush_buf()?;
        }
        if buf.len() >= self.buf.capacity() {
            self.inner.write(buf)
        } else {
            self.buf.extend_from_slice(buf);
            Ok(buf.len())
        }
    }

    /// The slow path of `write_all`, as for `write`.
    #[cold]
    #[inline(never)]
    fn write_all_cold(&mut self, buf: &[u8]) -> Result<()> {
        if buf.len() > self.spare_capacity() {
            self.flush_buf()?;
        }
        if buf.len() >= self.buf.capacity() {
            self.inner.write_all(buf)
        } else {
            self.buf.extend_from_slice(buf);
            Ok(())
        }
    }
}

/// Buffers writes smaller than the buffer; larger ones go straight to the
/// underlying writer once the buffer is flushed.
impl<W: ?Sized + Write> Write for BufWriter<W> {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        // `<` rather than `<=`: data that would fill the buffer exactly
        // must be flushed anyway, so it goes to `write_cold`.
        if buf.len() < self.spare_capacity() {
            self.buf.extend_from_slice(buf);
            Ok(buf.len())
        } else {
            self.write_cold(buf)
        }
    }

    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        if buf.len() < self.spare_capacity() {
            self.buf.extend_from_slice(buf);
            Ok(())
        } else {
            self.write_all_cold(buf)
        }
    }

    /// Buffers the first nonempty buffer as `write` would, then as many of
    /// the following buffers as fit completely.
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> Result<usize> {
        let mut rest = bufs.iter().skip_while(|b| b.is_empty());
        let Some(first) = rest.next() else {
            return Ok(0);
        };
        if first.len() > self.spare_capacity() {
            self.flush_buf()?;
        }
        if first.len() >= self.buf.capacity() {
            return self.inner.write(first);
        }
        self.buf.extend_from_slice(first);
        let mut written = first.len();
        for buf in rest {
            if buf.len() > self.spare_capacity() {
                break;
            }
            self.buf.extend_from_slice(buf);
            // Everything written so far fits in the buffer.
            written += buf.len();
        }
        Ok(written)
    }

    fn flush(&mut self) -> Result<()> {
        self.flush_buf()?;
        self.inner.flush()
    }
}

impl<W: ?Sized + Write + fmt::Debug> fmt::Debug for BufWriter<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BufWriter")
            .field("writer", &&self.inner)
            .field(
                "buffer",
                &format_args!("{}/{}", self.buf.len(), self.buf.capacity()),
            )
            .finish()
    }
}

/// Seeking writes out the buffer before seeking the underlying writer.
impl<W: ?Sized + Write + Seek> Seek for BufWriter<W> {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        self.flush_buf()?;
        self.inner.seek(pos)
    }
}

/// Dropping a `BufWriter` writes out its buffer, ignoring any error.
impl<W: ?Sized + Write> Drop for BufWriter<W> {
    fn drop(&mut self) {
        // A destructor cannot report the error; call `flush` to see it.
        let _ = self.flush_buf();
    }
}

/// The error [`BufWriter::into_parts`] returns for the buffered data if the
/// writer panicked. Panics abort in litestd, so it never occurs.
pub struct WriterPanicked {
    buf: Vec<u8>,
}

impl WriterPanicked {
    /// Returns the perhaps-unwritten data.
    #[must_use = "`self` will be dropped if the result is not used"]
    pub fn into_inner(self) -> Vec<u8> {
        self.buf
    }
}

impl error::Error for WriterPanicked {}

impl fmt::Display for WriterPanicked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(
            "BufWriter inner writer panicked, what data remains unwritten is \
             not known",
            f,
        )
    }
}

impl fmt::Debug for WriterPanicked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WriterPanicked")
            .field(
                "buffer",
                &format_args!("{}/{}", self.buf.len(), self.buf.capacity()),
            )
            .finish()
    }
}
