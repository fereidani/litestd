//! [`BufReader`].

use core::{fmt, iter};

use alloc_crate::{boxed::Box, string::String, vec::Vec};

use crate::io::{
    BufRead, DEFAULT_BUF_SIZE, Error, IoSliceMut, Read, Result, Seek, SeekFrom,
    append_to_string, impls::copy_prefix,
};

/// The `BufReader<R>` struct adds buffering to any reader.
///
/// It makes small, repeated reads cheap by reading the inner reader in large,
/// infrequent chunks. Its buffered data is lost when it is dropped or
/// unwrapped with [`BufReader::into_inner`].
pub struct BufReader<R: ?Sized> {
    buf: Buffer,
    inner: R,
}

/// The buffer of a [`BufReader`].
///
/// Invariant: `pos <= filled <= buf.len()`. `buf[pos..filled]` holds the
/// data not yet consumed.
struct Buffer {
    /// Zeroed on allocation, so that it can be lent to readers.
    buf: Box<[u8]>,
    pos: usize,
    filled: usize,
}

impl Buffer {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            // Exactly `capacity` zeroed bytes, as the caller asked for.
            buf: iter::repeat_n(0, capacity).collect(),
            pos: 0,
            filled: 0,
        }
    }

    /// The data not yet consumed.
    #[inline]
    fn buffer(&self) -> &[u8] {
        self.buf.get(self.pos..self.filled).unwrap_or_default()
    }

    #[inline]
    const fn discard(&mut self) {
        self.pos = 0;
        self.filled = 0;
    }

    #[inline]
    fn consume(&mut self, amount: usize) {
        self.pos = self.pos.saturating_add(amount).min(self.filled);
    }

    /// Calls `f` with the next `amount` bytes and consumes them, if that
    /// many are buffered.
    #[inline]
    fn consume_with(&mut self, amount: usize, f: impl FnOnce(&[u8])) -> bool {
        match self.buffer().get(..amount) {
            Some(claimed) => {
                f(claimed);
                self.pos += amount;
                true
            }
            None => false,
        }
    }

    /// Returns the buffered data, refilling the buffer from `reader` first
    /// if all of it was consumed.
    #[inline]
    fn fill_buf<R: Read + ?Sized>(&mut self, reader: &mut R) -> Result<&[u8]> {
        if self.pos >= self.filled {
            // A failed read leaves the buffer empty.
            self.discard();
            let n = reader.read(&mut self.buf)?;
            // A count beyond the buffer breaks `read`'s contract; clamping
            // it keeps the invariant.
            self.filled = n.min(self.buf.len());
        }
        Ok(self.buffer())
    }
}

impl<R: Read> BufReader<R> {
    /// Creates a new `BufReader<R>` with a default buffer capacity. The
    /// default is currently 8 KiB, but may change in the future.
    pub fn new(inner: R) -> Self {
        Self::with_capacity(DEFAULT_BUF_SIZE, inner)
    }

    /// Creates a new `BufReader<R>` with the specified buffer capacity.
    pub fn with_capacity(capacity: usize, inner: R) -> Self {
        Self {
            buf: Buffer::with_capacity(capacity),
            inner,
        }
    }
}

impl<R: ?Sized> BufReader<R> {
    /// Gets a reference to the underlying reader.
    #[allow(clippy::missing_const_for_fn, reason = "not `const` in std")]
    pub fn get_ref(&self) -> &R {
        &self.inner
    }

    /// Gets a mutable reference to the underlying reader.
    #[allow(clippy::missing_const_for_fn, reason = "not `const` in std")]
    pub fn get_mut(&mut self) -> &mut R {
        &mut self.inner
    }

    /// Returns a reference to the internally buffered data, without filling
    /// the buffer as [`fill_buf`](BufRead::fill_buf) would.
    pub fn buffer(&self) -> &[u8] {
        self.buf.buffer()
    }

    /// Returns the number of bytes the internal buffer can hold at once.
    pub fn capacity(&self) -> usize {
        self.buf.buf.len()
    }

    /// Unwraps this `BufReader<R>`, returning the underlying reader; any
    /// buffered data is lost.
    pub fn into_inner(self) -> R
    where
        R: Sized,
    {
        self.inner
    }

    /// Whether the buffer is consumed and `len` bytes would not fit in it,
    /// so that a read of `len` bytes should bypass it.
    fn bypass(&self, len: usize) -> bool {
        self.buf.pos == self.buf.filled && len >= self.capacity()
    }
}

impl<R: ?Sized + Seek> BufReader<R> {
    /// Seeks relative to the current position, keeping the buffer if the new
    /// position lies within it.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Seek::seek`] on the underlying reader, which is
    /// called only if the new position lies outside the buffer.
    pub fn seek_relative(&mut self, offset: i64) -> Result<()> {
        // Backwards, the new position is below `pos`, hence within `filled`.
        let within = isize::try_from(offset)
            .ok()
            .and_then(|d| self.buf.pos.checked_add_signed(d))
            .filter(|&pos| pos <= self.buf.filled);
        if let Some(pos) = within {
            self.buf.pos = pos;
            return Ok(());
        }
        self.seek(SeekFrom::Current(offset)).map(drop)
    }
}

impl<R: ?Sized + Read> Read for BufReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        // With nothing buffered, a read at least as large as the buffer
        // skips it.
        if self.bypass(buf.len()) {
            self.buf.discard();
            return self.inner.read(buf);
        }
        let n = copy_prefix(buf, self.fill_buf()?);
        self.consume(n);
        Ok(n)
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> Result<usize> {
        // `IoSliceMut`s do not alias, so their total fits in a `usize`.
        let total = bufs.iter().map(|b| b.len()).fold(0, usize::saturating_add);
        if self.bypass(total) {
            self.buf.discard();
            return self.inner.read_vectored(bufs);
        }
        let n = self.fill_buf()?.read_vectored(bufs)?;
        self.consume(n);
        Ok(n)
    }

    // Small exact reads are common with deserializers; this serves them from
    // the buffer without the loop of the provided method.
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        if self.buf.consume_with(buf.len(), |claimed| {
            copy_prefix(buf, claimed);
        }) {
            return Ok(());
        }
        crate::io::read::default_read_exact(self, buf)
    }

    // The inner reader may have a faster `read_to_end`: drain the buffer and
    // delegate to it.
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
        let buffered = self.buf.buffer();
        buf.try_reserve(buffered.len())?;
        buf.extend_from_slice(buffered);
        let n = buffered.len();
        self.buf.discard();
        Ok(n + self.inner.read_to_end(buf)?)
    }

    /// When `buf` is not empty, the bytes read go through a separate buffer
    /// to be validated, and are lost if a read fails.
    fn read_to_string(&mut self, buf: &mut String) -> Result<usize> {
        if buf.is_empty() {
            // SAFETY: `buf` is empty, so there is nothing `read_to_end` could
            // change or remove.
            return unsafe {
                append_to_string(buf, |bytes| self.read_to_end(bytes))
            };
        }
        // A partial UTF-8 sequence may be buffered, so validate everything
        // read, then append it.
        let mut bytes = Vec::new();
        self.read_to_end(&mut bytes)?;
        let text =
            core::str::from_utf8(&bytes).map_err(|_| Error::INVALID_UTF8)?;
        buf.push_str(text);
        Ok(text.len())
    }
}

impl<R: ?Sized + Read> BufRead for BufReader<R> {
    fn fill_buf(&mut self) -> Result<&[u8]> {
        self.buf.fill_buf(&mut self.inner)
    }

    fn consume(&mut self, amount: usize) {
        self.buf.consume(amount);
    }
}

#[allow(clippy::missing_fields_in_debug, reason = "std's format")]
impl<R: ?Sized + fmt::Debug> fmt::Debug for BufReader<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BufReader")
            .field("reader", &&self.inner)
            .field(
                "buffer",
                &format_args!("{}/{}", self.buffer().len(), self.capacity()),
            )
            .finish()
    }
}

/// Seeking discards the buffer. A relative seek accounts for the buffered
/// data, which the underlying reader has already moved past.
impl<R: ?Sized + Seek> Seek for BufReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        let result = if let SeekFrom::Current(n) = pos {
            // The buffer is at most `isize::MAX` bytes.
            let remainder =
                i64::try_from(self.buf.buffer().len()).unwrap_or(i64::MAX);
            if let Some(offset) = n.checked_sub(remainder) {
                self.inner.seek(SeekFrom::Current(offset))?
            } else {
                // Seek back over the buffered data first, then by `n`.
                self.inner.seek(SeekFrom::Current(-remainder))?;
                self.buf.discard();
                self.inner.seek(SeekFrom::Current(n))?
            }
        } else {
            self.inner.seek(pos)?
        };
        self.buf.discard();
        Ok(result)
    }

    /// # Panics
    ///
    /// Panics if the position of the inner reader is smaller than the amount
    /// of buffered data, which only a faulty or directly seeked reader causes.
    fn stream_position(&mut self) -> Result<u64> {
        let remainder = self.buf.buffer().len() as u64;
        let pos = self.inner.stream_position()?;
        let Some(pos) = pos.checked_sub(remainder) else {
            position_underflow();
        };
        Ok(pos)
    }

    fn seek_relative(&mut self, offset: i64) -> Result<()> {
        Self::seek_relative(self, offset)
    }
}

// `BufReader::stream_position` documents this panic.
#[cold]
#[inline(never)]
#[track_caller]
#[allow(clippy::panic, reason = "std documents this panic")]
fn position_underflow() -> ! {
    panic!(
        "overflow when subtracting remaining buffer size from inner stream \
         position"
    )
}
