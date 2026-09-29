//! [`LineWriter`].

use core::fmt;

use crate::io::{
    BufWriter, IntoInnerError, IoSlice, Result, Write, memchr::memrchr,
    write::first_nonempty,
};

/// Wraps a writer and buffers output to it, flushing whenever a newline
/// (`0x0a`, `'\n'`) is detected.
///
/// Like [`BufWriter`], it also flushes when its buffer is full and when it is
/// dropped.
pub struct LineWriter<W: ?Sized + Write> {
    inner: BufWriter<W>,
}

impl<W: Write> LineWriter<W> {
    /// Creates a new `LineWriter`, with a buffer of 1 KiB.
    pub fn new(inner: W) -> Self {
        // Lines are usually short; a large buffer would be wasted.
        Self::with_capacity(1024, inner)
    }

    /// Creates a new `LineWriter` with at least the specified capacity for
    /// the internal buffer.
    pub fn with_capacity(capacity: usize, inner: W) -> Self {
        Self {
            inner: BufWriter::with_capacity(capacity, inner),
        }
    }

    /// Gets a mutable reference to the underlying writer.
    pub fn get_mut(&mut self) -> &mut W {
        self.inner.get_mut()
    }

    /// Unwraps this `LineWriter`, returning the underlying writer once the
    /// buffer is written out.
    ///
    /// # Errors
    ///
    /// Fails with the error and the `LineWriter` if flushing the buffer fails.
    pub fn into_inner(self) -> core::result::Result<W, IntoInnerError<Self>> {
        self.inner.into_inner().map_err(|err| {
            let (error, inner) = err.into_parts();
            IntoInnerError(Self { inner }, error)
        })
    }
}

impl<W: ?Sized + Write> LineWriter<W> {
    /// Gets a reference to the underlying writer.
    pub fn get_ref(&self) -> &W {
        self.inner.get_ref()
    }
}

/// Writes whole lines through to the underlying writer and buffers the
/// incomplete line at the end, flushing it once it is completed.
impl<W: ?Sized + Write> Write for LineWriter<W> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        line_write(&mut self.inner, buf)
    }

    /// Writes the first nonempty buffer as `write` would.
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> Result<usize> {
        match first_nonempty(bufs) {
            [] => Ok(0),
            buf => line_write(&mut self.inner, buf),
        }
    }

    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        line_write_all(&mut self.inner, buf)
    }

    fn flush(&mut self) -> Result<()> {
        self.inner.flush()
    }
}

impl<W: ?Sized + Write + fmt::Debug> fmt::Debug for LineWriter<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LineWriter")
            .field("writer", &self.get_ref())
            .field(
                "buffer",
                &format_args!(
                    "{}/{}",
                    self.inner.buffer().len(),
                    self.inner.capacity()
                ),
            )
            .finish_non_exhaustive()
    }
}

/// Flushes `buffer` if it holds a complete line, so that a new line does
/// not wait behind it.
fn flush_if_completed_line<W: ?Sized + Write>(
    buffer: &mut BufWriter<W>,
) -> Result<()> {
    match buffer.buffer().last() {
        Some(b'\n') => buffer.flush_buf(),
        _ => Ok(()),
    }
}

/// `LineWriter::write`: writes the lines of `buf` with at most one write to
/// the underlying writer and buffers what follows the last newline. Errors
/// from flushing the buffer come first, so that a success never hides one.
fn line_write<W: ?Sized + Write>(
    buffer: &mut BufWriter<W>,
    buf: &[u8],
) -> Result<usize> {
    let Some(newline) = memrchr(b'\n', buf) else {
        // Less than a line: buffer it, which may flush a full buffer.
        flush_if_completed_line(buffer)?;
        return buffer.write(buf);
    };
    buffer.flush_buf()?;
    let lines_end = newline + 1;
    let lines = buf.get(..lines_end).unwrap_or(buf);
    // A count beyond `lines` breaks `write`'s contract; clamp it.
    let flushed = buffer.get_mut().write(lines)?.min(lines.len());
    // Buffering after a write of nothing would only turn `Ok(0)` into a
    // `WriteZero` error later.
    if flushed == 0 {
        return Ok(0);
    }
    let unwritten = buf.get(flushed..).unwrap_or_default();
    let tail = if flushed >= lines_end {
        // Only a partial line is left; one too large to buffer is left for
        // the next call rather than split.
        if unwritten.len() >= buffer.capacity() {
            return Ok(flushed);
        }
        unwritten
    } else if lines_end - flushed <= buffer.capacity() {
        // The rest of the lines fits in the buffer; the partial line after
        // them waits for the next call.
        unwritten.get(..lines_end - flushed).unwrap_or(unwritten)
    } else {
        // Buffer up to the last newline that fits, or fill the buffer.
        let fits = unwritten.get(..buffer.capacity()).unwrap_or(unwritten);
        memrchr(b'\n', fits).map_or(fits, |i| fits.get(..=i).unwrap_or(fits))
    };
    Ok(flushed + buffer.write_to_buf(tail))
}

/// `LineWriter::write_all`: writes the lines of `buf` completely, adding
/// them to any buffered data to save a write, then buffers the rest.
fn line_write_all<W: ?Sized + Write>(
    buffer: &mut BufWriter<W>,
    buf: &[u8],
) -> Result<()> {
    let Some(newline) = memrchr(b'\n', buf) else {
        flush_if_completed_line(buffer)?;
        return buffer.write_all(buf);
    };
    let (lines, tail) = buf.split_at_checked(newline + 1).unwrap_or((buf, &[]));
    if buffer.buffer().is_empty() {
        buffer.get_mut().write_all(lines)?;
    } else {
        buffer.write_all(lines)?;
        buffer.flush_buf()?;
    }
    buffer.write_all(tail)
}
