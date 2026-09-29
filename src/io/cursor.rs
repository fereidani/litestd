//! [`Cursor`], an in-memory buffer with a position.

use alloc_crate::{boxed::Box, string::String, vec::Vec};

use super::{
    BufRead, Error, ErrorKind, IoSlice, IoSliceMut, Read, Result, Seek,
    SeekFrom, Write, const_error,
    impls::{copy_prefix, min_len, total_len},
};

/// A `Cursor` wraps an in-memory buffer and provides it with a [`Seek`]
/// implementation, so that it can be used as a reader or writer.
///
/// Writing to a `Vec<u8>` or `&mut Vec<u8>` grows it as needed; writing to a
/// fixed buffer (`&mut [u8]`, `[u8; N]` or `Box<[u8]>`) stops at its end.
///
/// # Examples
///
/// ```
/// use litestd::io::{self, Cursor, Seek, SeekFrom, Write};
///
/// let mut cursor = Cursor::new(Vec::new());
/// cursor.write_all(b"hello")?;
/// cursor.seek(SeekFrom::Start(1))?;
/// cursor.write_all(b"EL")?;
/// assert_eq!(cursor.get_ref(), b"hELlo");
/// # Ok::<(), io::Error>(())
/// ```
#[derive(Debug, Default, Eq, PartialEq)]
pub struct Cursor<T> {
    inner: T,
    pos: u64,
}

impl<T> Cursor<T> {
    /// Creates a new cursor wrapping the provided underlying in-memory buffer,
    /// at position 0 even if the buffer is not empty.
    pub const fn new(inner: T) -> Self {
        Self { inner, pos: 0 }
    }

    /// Consumes this cursor, returning the underlying value.
    pub fn into_inner(self) -> T {
        self.inner
    }

    /// Gets a reference to the underlying value in this cursor.
    pub const fn get_ref(&self) -> &T {
        &self.inner
    }

    /// Gets a mutable reference to the underlying value in this cursor.
    pub const fn get_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Returns the current position of this cursor.
    pub const fn position(&self) -> u64 {
        self.pos
    }

    /// Sets the position of this cursor. It may lie beyond the end of the
    /// buffer.
    pub const fn set_position(&mut self, pos: u64) {
        self.pos = pos;
    }
}

impl<T: Clone> Clone for Cursor<T> {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            pos: self.pos,
        }
    }

    #[inline]
    fn clone_from(&mut self, other: &Self) {
        self.inner.clone_from(&other.inner);
        self.pos = other.pos;
    }
}

/// Returns the bytes of `buf` from `pos` on, empty past the end.
#[inline]
fn remaining(buf: &[u8], pos: u64) -> &[u8] {
    buf.get(min_len(pos, buf.len())..).unwrap_or_default()
}

/// Advances `pos` past `n` bytes of the buffer that follow it. The buffer
/// ends at most at `usize::MAX`, so this never wraps.
#[inline]
const fn advance(pos: &mut u64, n: usize) {
    *pos = pos.wrapping_add(n as u64);
}

impl<T: AsRef<[u8]>> Read for Cursor<T> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let n = copy_prefix(buf, remaining(self.inner.as_ref(), self.pos));
        advance(&mut self.pos, n);
        Ok(n)
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> Result<usize> {
        let mut read = 0;
        for buf in bufs {
            let n = self.read(buf)?;
            read += n;
            if n < buf.len() {
                break;
            }
        }
        Ok(read)
    }

    /// Fails without copying anything if too few bytes remain, leaving the
    /// position at the end of the buffer.
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        let data = self.inner.as_ref();
        let Some(src) = remaining(data, self.pos).get(..buf.len()) else {
            self.pos = data.len() as u64;
            return Err(Error::READ_EXACT_EOF);
        };
        copy_prefix(buf, src);
        advance(&mut self.pos, buf.len());
        Ok(())
    }

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
        let n = remaining(self.inner.as_ref(), self.pos).read_to_end(buf)?;
        advance(&mut self.pos, n);
        Ok(n)
    }

    /// Fails without consuming anything if the remaining bytes are not
    /// valid UTF-8.
    fn read_to_string(&mut self, buf: &mut String) -> Result<usize> {
        let n = remaining(self.inner.as_ref(), self.pos).read_to_string(buf)?;
        advance(&mut self.pos, n);
        Ok(n)
    }
}

impl<T: AsRef<[u8]>> BufRead for Cursor<T> {
    fn fill_buf(&mut self) -> Result<&[u8]> {
        Ok(remaining(self.inner.as_ref(), self.pos))
    }

    fn consume(&mut self, amount: usize) {
        // The amount is the caller's, which `consume` does not check against
        // the buffer, so the position saturates instead of wrapping.
        self.pos = self.pos.saturating_add(amount as u64);
    }
}

impl<T: AsRef<[u8]>> Seek for Cursor<T> {
    fn seek(&mut self, style: SeekFrom) -> Result<u64> {
        let (base, offset) = match style {
            SeekFrom::Start(n) => {
                self.pos = n;
                return Ok(n);
            }
            SeekFrom::End(n) => (self.inner.as_ref().len() as u64, n),
            SeekFrom::Current(n) => (self.pos, n),
        };
        match base.checked_add_signed(offset) {
            Some(n) => {
                self.pos = n;
                Ok(n)
            }
            None => Err(const_error!(
                ErrorKind::InvalidInput,
                "invalid seek to a negative or overflowing position",
            )),
        }
    }

    fn stream_position(&mut self) -> Result<u64> {
        Ok(self.pos)
    }
}

/// Writes `buf` into the fixed buffer `slice` at `pos`, as much as fits.
fn slice_write(pos: &mut u64, slice: &mut [u8], buf: &[u8]) -> usize {
    let start = min_len(*pos, slice.len());
    let n = copy_prefix(slice.get_mut(start..).unwrap_or_default(), buf);
    advance(pos, n);
    n
}

/// Writes each buffer of `bufs` into `slice` at `pos` until one does not
/// fit.
fn slice_write_vectored(
    pos: &mut u64,
    slice: &mut [u8],
    bufs: &[IoSlice<'_>],
) -> usize {
    let mut written = 0;
    for buf in bufs {
        let n = slice_write(pos, slice, buf);
        written += n;
        if n < buf.len() {
            break;
        }
    }
    written
}

/// Writes all of `buf` into `slice` at `pos`, failing if it does not fit.
fn slice_write_all(pos: &mut u64, slice: &mut [u8], buf: &[u8]) -> Result<()> {
    if slice_write(pos, slice, buf) < buf.len() {
        Err(Error::WRITE_ALL_EOF)
    } else {
        Ok(())
    }
}

/// Makes room in `vec` for `len` bytes at `pos`, zero-filling any gap
/// between its end and `pos`, and returns `pos` as an index.
fn reserve_and_pad(pos: u64, vec: &mut Vec<u8>, len: usize) -> Result<usize> {
    let Ok(pos) = usize::try_from(pos) else {
        return Err(const_error!(
            ErrorKind::InvalidInput,
            "cursor position exceeds maximum possible vector length",
        ));
    };
    let end = pos.saturating_add(len);
    if end > vec.capacity() {
        vec.reserve(end - vec.len());
    }
    if pos > vec.len() {
        vec.resize(pos, 0);
    }
    Ok(pos)
}

/// Overwrites and extends `vec` with `buf` from index `pos`, which is at
/// most `vec.len()`.
fn vec_put(vec: &mut Vec<u8>, pos: usize, buf: &[u8]) {
    let overlap = copy_prefix(vec.get_mut(pos..).unwrap_or_default(), buf);
    vec.extend_from_slice(buf.get(overlap..).unwrap_or_default());
}

/// Writes all of `buf` into `vec` at `pos`, growing it as needed.
fn vec_write(pos: &mut u64, vec: &mut Vec<u8>, buf: &[u8]) -> Result<usize> {
    let at = reserve_and_pad(*pos, vec, buf.len())?;
    vec_put(vec, at, buf);
    advance(pos, buf.len());
    Ok(buf.len())
}

/// Writes all of `bufs` into `vec` at `pos`, growing it as needed.
fn vec_write_vectored(
    pos: &mut u64,
    vec: &mut Vec<u8>,
    bufs: &[IoSlice<'_>],
) -> Result<usize> {
    let len = total_len(bufs);
    let mut at = reserve_and_pad(*pos, vec, len)?;
    for buf in bufs {
        vec_put(vec, at, buf);
        at += buf.len();
    }
    advance(pos, len);
    Ok(len)
}

/// Implements `Write` for cursors over fixed buffers.
macro_rules! slice_cursor_write {
    () => {
        #[inline]
        fn write(&mut self, buf: &[u8]) -> Result<usize> {
            Ok(slice_write(&mut self.pos, &mut self.inner[..], buf))
        }

        #[inline]
        fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> Result<usize> {
            Ok(slice_write_vectored(
                &mut self.pos,
                &mut self.inner[..],
                bufs,
            ))
        }

        #[inline]
        fn write_all(&mut self, buf: &[u8]) -> Result<()> {
            slice_write_all(&mut self.pos, &mut self.inner[..], buf)
        }

        #[inline]
        fn flush(&mut self) -> Result<()> {
            Ok(())
        }
    };
}

/// Implements `Write` for cursors over vectors.
macro_rules! vec_cursor_write {
    () => {
        #[inline]
        fn write(&mut self, buf: &[u8]) -> Result<usize> {
            vec_write(&mut self.pos, &mut self.inner, buf)
        }

        #[inline]
        fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> Result<usize> {
            vec_write_vectored(&mut self.pos, &mut self.inner, bufs)
        }

        #[inline]
        fn write_all(&mut self, buf: &[u8]) -> Result<()> {
            vec_write(&mut self.pos, &mut self.inner, buf).map(drop)
        }

        #[inline]
        fn flush(&mut self) -> Result<()> {
            Ok(())
        }
    };
}

impl Write for Cursor<&mut [u8]> {
    slice_cursor_write!();
}

impl<const N: usize> Write for Cursor<[u8; N]> {
    slice_cursor_write!();
}

impl Write for Cursor<Box<[u8]>> {
    slice_cursor_write!();
}

impl Write for Cursor<Vec<u8>> {
    vec_cursor_write!();
}

// The shared body borrows `self.inner`, a `&mut Vec<u8>` here, mutably.
#[allow(clippy::mut_mut)]
impl Write for Cursor<&mut Vec<u8>> {
    vec_cursor_write!();
}
