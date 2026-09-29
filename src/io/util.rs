//! The adapters and helpers: [`Chain`], [`Take`], [`Bytes`], [`Lines`],
//! [`Split`], [`copy`], [`Empty`], [`Repeat`] and [`Sink`].

use core::{fmt, slice};

use alloc_crate::{string::String, vec::Vec};

use super::{
    BufRead, DEFAULT_BUF_SIZE, Error, ErrorKind, IoSlice, IoSliceMut, Read,
    Result, Seek, SeekFrom, Write,
    impls::{min_len, total_len},
};

/// Copies the entire contents of a reader into a writer, returning the
/// number of bytes copied.
///
/// # Errors
///
/// Returns the first error of a `read` or `write` call other than
/// [`ErrorKind::Interrupted`], which is retried.
pub fn copy<R, W>(reader: &mut R, writer: &mut W) -> Result<u64>
where
    R: Read + ?Sized,
    W: Write + ?Sized,
{
    let mut buf = [0; DEFAULT_BUF_SIZE];
    let mut copied = 0_u64;
    // Ends at EOF or on an error other than an interrupted read.
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => return Ok(copied),
            Ok(n) => n,
            Err(e) if e.is_interrupted() => continue,
            Err(e) => return Err(e),
        };
        let data = buf.get(..n).unwrap_or(&buf);
        writer.write_all(data)?;
        copied = copied.saturating_add(data.len() as u64);
    }
}

/// A reader that is always at EOF and a writer that discards everything,
/// created by [`empty()`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default)]
pub struct Empty;

/// Creates an [`Empty`]: always at EOF for reads, ignoring all writes.
#[must_use]
pub const fn empty() -> Empty {
    Empty
}

impl Read for Empty {
    #[inline]
    fn read(&mut self, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }

    #[inline]
    fn read_vectored(&mut self, _bufs: &mut [IoSliceMut<'_>]) -> Result<usize> {
        Ok(0)
    }

    #[inline]
    fn read_to_end(&mut self, _buf: &mut Vec<u8>) -> Result<usize> {
        Ok(0)
    }

    #[inline]
    fn read_to_string(&mut self, _buf: &mut String) -> Result<usize> {
        Ok(0)
    }

    #[inline]
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        if buf.is_empty() {
            Ok(())
        } else {
            Err(Error::READ_EXACT_EOF)
        }
    }
}

impl BufRead for Empty {
    #[inline]
    fn fill_buf(&mut self) -> Result<&[u8]> {
        Ok(&[])
    }

    #[inline]
    fn consume(&mut self, _amount: usize) {}

    #[inline]
    fn read_until(&mut self, _byte: u8, _buf: &mut Vec<u8>) -> Result<usize> {
        Ok(0)
    }

    #[inline]
    fn skip_until(&mut self, _byte: u8) -> Result<usize> {
        Ok(0)
    }

    #[inline]
    fn read_line(&mut self, _buf: &mut String) -> Result<usize> {
        Ok(0)
    }
}

/// Seeking an `Empty` always succeeds and stays at position 0.
impl Seek for Empty {
    #[inline]
    fn seek(&mut self, _pos: SeekFrom) -> Result<u64> {
        Ok(0)
    }

    #[inline]
    fn stream_position(&mut self) -> Result<u64> {
        Ok(0)
    }
}

/// Implements `Write` for a type that accepts and discards everything.
macro_rules! discarding_write {
    () => {
        #[inline]
        fn write(&mut self, buf: &[u8]) -> Result<usize> {
            Ok(buf.len())
        }

        #[inline]
        fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> Result<usize> {
            Ok(total_len(bufs))
        }

        #[inline]
        fn write_all(&mut self, _buf: &[u8]) -> Result<()> {
            Ok(())
        }

        #[inline]
        fn write_fmt(&mut self, _args: fmt::Arguments<'_>) -> Result<()> {
            Ok(())
        }

        #[inline]
        fn flush(&mut self) -> Result<()> {
            Ok(())
        }
    };
}

impl Write for Empty {
    discarding_write!();
}

impl Write for &Empty {
    discarding_write!();
}

/// A reader which yields one byte over and over, created by [`repeat()`].
pub struct Repeat {
    byte: u8,
}

/// Creates a reader that fills every buffer with `byte`, forever.
#[must_use]
pub const fn repeat(byte: u8) -> Repeat {
    Repeat { byte }
}

/// Reading to the end of a `Repeat` fails with [`ErrorKind::OutOfMemory`],
/// since the data never ends.
impl Read for Repeat {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        buf.fill(self.byte);
        Ok(buf.len())
    }

    #[inline]
    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> Result<usize> {
        let mut read = 0;
        for buf in bufs {
            buf.fill(self.byte);
            read += buf.len();
        }
        Ok(read)
    }

    fn read_to_end(&mut self, _buf: &mut Vec<u8>) -> Result<usize> {
        Err(ErrorKind::OutOfMemory.into())
    }

    fn read_to_string(&mut self, _buf: &mut String) -> Result<usize> {
        Err(ErrorKind::OutOfMemory.into())
    }

    #[inline]
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        buf.fill(self.byte);
        Ok(())
    }
}

impl fmt::Debug for Repeat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Repeat").finish_non_exhaustive()
    }
}

/// A writer which will move data into the void, created by [`sink()`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default)]
pub struct Sink;

/// Creates a writer that accepts and discards all data.
#[must_use]
pub const fn sink() -> Sink {
    Sink
}

impl Write for Sink {
    discarding_write!();
}

impl Write for &Sink {
    discarding_write!();
}

/// Adapter to chain together two readers, created by [`Read::chain`].
#[derive(Debug)]
pub struct Chain<T, U> {
    first: T,
    second: U,
    done_first: bool,
}

impl<T, U> Chain<T, U> {
    pub(super) const fn new(first: T, second: U) -> Self {
        Self {
            first,
            second,
            done_first: false,
        }
    }

    /// Consumes the `Chain`, returning the wrapped readers.
    pub fn into_inner(self) -> (T, U) {
        (self.first, self.second)
    }

    /// Gets references to the underlying readers in this `Chain`.
    #[allow(clippy::missing_const_for_fn, reason = "not `const` in std")]
    pub fn get_ref(&self) -> (&T, &U) {
        (&self.first, &self.second)
    }

    /// Gets mutable references to the underlying readers in this `Chain`.
    #[allow(clippy::missing_const_for_fn, reason = "not `const` in std")]
    pub fn get_mut(&mut self) -> (&mut T, &mut U) {
        (&mut self.first, &mut self.second)
    }
}

/// A read of the first reader that returns no bytes for a nonempty buffer
/// switches the chain to the second reader.
impl<T: Read, U: Read> Read for Chain<T, U> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if !self.done_first {
            match self.first.read(buf)? {
                0 if !buf.is_empty() => self.done_first = true,
                n => return Ok(n),
            }
        }
        self.second.read(buf)
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> Result<usize> {
        if !self.done_first {
            match self.first.read_vectored(bufs)? {
                0 if bufs.iter().any(|b| !b.is_empty()) => {
                    self.done_first = true;
                }
                n => return Ok(n),
            }
        }
        self.second.read_vectored(bufs)
    }

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
        let mut read = 0;
        if !self.done_first {
            read += self.first.read_to_end(buf)?;
            self.done_first = true;
        }
        Ok(read + self.second.read_to_end(buf)?)
    }

    // `read_to_string` keeps the provided method: a UTF-8 sequence may be
    // split between the two readers.
}

impl<T: BufRead, U: BufRead> BufRead for Chain<T, U> {
    fn fill_buf(&mut self) -> Result<&[u8]> {
        if !self.done_first {
            match self.first.fill_buf()? {
                [] => self.done_first = true,
                buf => return Ok(buf),
            }
        }
        self.second.fill_buf()
    }

    fn consume(&mut self, amount: usize) {
        if self.done_first {
            self.second.consume(amount);
        } else {
            self.first.consume(amount);
        }
    }

    fn read_until(&mut self, byte: u8, buf: &mut Vec<u8>) -> Result<usize> {
        let mut read = 0;
        if !self.done_first {
            read = self.first.read_until(byte, buf)?;
            if read != 0 && buf.last() == Some(&byte) {
                return Ok(read);
            }
            self.done_first = true;
        }
        Ok(read + self.second.read_until(byte, buf)?)
    }

    // `read_line` keeps the provided method: a UTF-8 sequence may be split
    // between the two readers.
}

/// Reader adapter which limits the bytes read from an underlying reader,
/// created by [`Read::take`].
#[derive(Debug)]
pub struct Take<T> {
    inner: T,
    /// The limit last set; the position is `len - limit`.
    len: u64,
    limit: u64,
}

impl<T> Take<T> {
    pub(super) const fn new(inner: T, limit: u64) -> Self {
        Self {
            inner,
            len: limit,
            limit,
        }
    }

    /// Returns the number of bytes this `Take` can read before returning EOF.
    #[allow(clippy::missing_const_for_fn, reason = "not `const` in std")]
    pub fn limit(&self) -> u64 {
        self.limit
    }

    /// Sets the number of bytes this `Take` can read before returning EOF, as
    /// if it were created anew.
    #[allow(clippy::missing_const_for_fn, reason = "not `const` in std")]
    pub fn set_limit(&mut self, limit: u64) {
        self.len = limit;
        self.limit = limit;
    }

    /// Consumes the `Take`, returning the wrapped reader.
    pub fn into_inner(self) -> T {
        self.inner
    }

    /// Gets a reference to the underlying reader.
    #[allow(clippy::missing_const_for_fn, reason = "not `const` in std")]
    pub fn get_ref(&self) -> &T {
        &self.inner
    }

    /// Gets a mutable reference to the underlying reader.
    #[allow(clippy::missing_const_for_fn, reason = "not `const` in std")]
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// The position within the limited stream.
    const fn position(&self) -> u64 {
        debug_assert!(self.limit <= self.len);
        self.len - self.limit
    }
}

/// At the limit, reads return EOF without calling the inner reader, which
/// might block.
impl<T: Read> Read for Take<T> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.limit == 0 {
            return Ok(0);
        }
        let max = min_len(self.limit, buf.len());
        let buf = buf.get_mut(..max).unwrap_or_default();
        // A count beyond the buffer, which breaks `read`'s contract, is
        // clamped so that the limit stays exact.
        let n = self.inner.read(buf)?.min(max);
        self.limit -= n as u64;
        Ok(n)
    }
}

impl<T: BufRead> BufRead for Take<T> {
    fn fill_buf(&mut self) -> Result<&[u8]> {
        if self.limit == 0 {
            return Ok(&[]);
        }
        let limit = self.limit;
        let buf = self.inner.fill_buf()?;
        Ok(buf.get(..min_len(limit, buf.len())).unwrap_or(buf))
    }

    /// An `amount` beyond the limit is clamped to it.
    fn consume(&mut self, amount: usize) {
        let amount = min_len(self.limit, amount);
        self.limit -= amount as u64;
        self.inner.consume(amount);
    }
}

/// Seeking moves within the first [`limit`](Take::limit) bytes, as last
/// set, of the inner reader from where it was at that time, with relative
/// seeks of the inner reader.
impl<T: Seek> Seek for Take<T> {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        let target = match pos {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::Current(n) => self.position().checked_add_signed(n),
            SeekFrom::End(n) => self.len.checked_add_signed(n),
        };
        let target = match target {
            Some(target) if target <= self.len => target,
            _ => return Err(ErrorKind::InvalidInput.into()),
        };
        // Any distance between two `u64`s is covered by at most two steps of
        // `i64::MAX` and a final one that fits.
        let mut steps = 3_u8;
        while steps > 0 {
            steps -= 1;
            let position = self.position();
            let offset = if target >= position {
                i64::try_from(target - position).unwrap_or(i64::MAX)
            } else {
                0_i64
                    .checked_sub_unsigned(position - target)
                    .unwrap_or(i64::MIN)
            };
            if offset == 0 {
                break;
            }
            self.seek_inner(offset)?;
        }
        Ok(target)
    }

    fn stream_position(&mut self) -> Result<u64> {
        Ok(self.position())
    }

    fn seek_relative(&mut self, offset: i64) -> Result<()> {
        match self.position().checked_add_signed(offset) {
            Some(target) if target <= self.len => {}
            _ => return Err(ErrorKind::InvalidInput.into()),
        }
        self.seek_inner(offset)
    }
}

impl<T: Seek> Take<T> {
    /// Moves the inner reader by `offset`, which keeps the position within
    /// `0..=len`, and the limit with it.
    fn seek_inner(&mut self, offset: i64) -> Result<()> {
        self.inner.seek_relative(offset)?;
        // Subtracting the offset's two's complement bit pattern wraps to
        // the exact new limit, which lies in `0..=len`.
        #[allow(clippy::cast_sign_loss, reason = "see above")]
        let offset = offset as u64;
        self.limit = self.limit.wrapping_sub(offset);
        Ok(())
    }
}

/// An iterator over the bytes of a reader, created by [`Read::bytes`].
#[derive(Debug)]
pub struct Bytes<R> {
    pub(super) inner: R,
}

impl<R: Read> Iterator for Bytes<R> {
    type Item = Result<u8>;

    fn next(&mut self) -> Option<Result<u8>> {
        let mut byte = 0;
        // Ends with a byte, at EOF, or on an error other than an
        // interruption.
        loop {
            return match self.inner.read(slice::from_mut(&mut byte)) {
                Ok(0) => None,
                Ok(_) => Some(Ok(byte)),
                Err(e) if e.is_interrupted() => continue,
                Err(e) => Some(Err(e)),
            };
        }
    }
}

/// An iterator over the contents of a `BufRead` split on a byte, created by
/// [`BufRead::split`].
#[derive(Debug)]
pub struct Split<B> {
    pub(super) buf: B,
    pub(super) delim: u8,
}

impl<B: BufRead> Iterator for Split<B> {
    type Item = Result<Vec<u8>>;

    fn next(&mut self) -> Option<Result<Vec<u8>>> {
        let mut buf = Vec::new();
        match self.buf.read_until(self.delim, &mut buf) {
            Ok(0) => None,
            Ok(_) => {
                let len = match buf.as_slice() {
                    [rest @ .., last] if *last == self.delim => rest.len(),
                    all => all.len(),
                };
                buf.truncate(len);
                Some(Ok(buf))
            }
            Err(e) => Some(Err(e)),
        }
    }
}

/// An iterator over the lines of a `BufRead`, created by [`BufRead::lines`].
#[derive(Debug)]
pub struct Lines<B> {
    pub(super) buf: B,
}

impl<B: BufRead> Iterator for Lines<B> {
    type Item = Result<String>;

    fn next(&mut self) -> Option<Result<String>> {
        let mut buf = String::new();
        match self.buf.read_line(&mut buf) {
            Ok(0) => None,
            Ok(_) => {
                // Both endings are ASCII, so cutting them keeps the string
                // valid UTF-8.
                let len = match buf.as_bytes() {
                    [line @ .., b'\r', b'\n'] | [line @ .., b'\n'] => {
                        line.len()
                    }
                    line => line.len(),
                };
                buf.truncate(len);
                Some(Ok(buf))
            }
            Err(e) => Some(Err(e)),
        }
    }
}
