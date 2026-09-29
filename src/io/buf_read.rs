//! The [`BufRead`] trait.

use alloc_crate::{string::String, vec::Vec};

use super::{Lines, Read, Result, Split, append_to_string, memchr::memchr};

/// A `BufRead` is a type of [`Read`]er which has an internal buffer, allowing
/// it to perform extra ways of reading, such as by line.
pub trait BufRead: Read {
    /// Returns the contents of the internal buffer, filling it from the inner
    /// reader if it is empty; an empty result means EOF.
    ///
    /// # Errors
    ///
    /// Returns the errors of the inner reader.
    fn fill_buf(&mut self) -> Result<&[u8]>;

    /// Marks `amount` bytes of the buffer returned by
    /// [`fill_buf`](BufRead::fill_buf) as read. The litestd implementations
    /// clamp an `amount` larger than the buffer to it.
    fn consume(&mut self, amount: usize);

    /// Reads all bytes into `buf` until the delimiter `byte` or EOF is
    /// reached, and returns how many bytes were read, the delimiter included.
    ///
    /// # Errors
    ///
    /// [`Interrupted`](super::ErrorKind::Interrupted) errors are retried; after
    /// any other error, `buf` holds the bytes read so far.
    fn read_until(&mut self, byte: u8, buf: &mut Vec<u8>) -> Result<usize> {
        read_until(self, byte, Some(buf))
    }

    /// Skips all bytes until the delimiter `byte` or EOF is reached, and
    /// returns how many bytes were skipped, the delimiter included.
    ///
    /// # Errors
    ///
    /// As for [`read_until`](BufRead::read_until).
    fn skip_until(&mut self, byte: u8) -> Result<usize> {
        read_until(self, byte, None)
    }

    /// Reads all bytes until a newline (the `0xA` byte), appending them to
    /// `buf` with the newline, and returns how many bytes were read, 0 at EOF.
    ///
    /// # Errors
    ///
    /// As for [`read_until`](BufRead::read_until), plus `InvalidData` for
    /// non-UTF-8 input; `buf` gets nothing unless all of it is valid UTF-8.
    fn read_line(&mut self, buf: &mut String) -> Result<usize> {
        // SAFETY: `read_until` only appends to the vector.
        unsafe { append_to_string(buf, |b| read_until(self, b'\n', Some(b))) }
    }

    /// Returns an iterator over the contents of this reader split on the byte
    /// `byte`, without the delimiter.
    fn split(self, byte: u8) -> Split<Self>
    where
        Self: Sized,
    {
        Split {
            buf: self,
            delim: byte,
        }
    }

    /// Returns an iterator over the lines of this reader, without the trailing
    /// `\n` or `\r\n`.
    fn lines(self) -> Lines<Self>
    where
        Self: Sized,
    {
        Lines { buf: self }
    }
}

/// The provided [`BufRead::read_until`], and [`BufRead::skip_until`] when
/// `buf` is `None`.
fn read_until<R: BufRead + ?Sized>(
    reader: &mut R,
    delim: u8,
    mut buf: Option<&mut Vec<u8>>,
) -> Result<usize> {
    let mut read = 0_usize;
    // Ends at the delimiter, at EOF, or on an error other than an
    // interruption.
    loop {
        let (done, used) = match reader.fill_buf() {
            Ok(available) => scan(available, delim, buf.as_deref_mut()),
            Err(e) if e.is_interrupted() => continue,
            Err(e) => return Err(e),
        };
        reader.consume(used);
        read = read.saturating_add(used);
        if done || used == 0 {
            return Ok(read);
        }
    }
}

/// Takes the bytes of `available` up to and including the first `delim`,
/// or all of them, appending them to `buf` if there is one. Returns
/// whether `delim` was found and how many bytes were taken.
fn scan(
    available: &[u8],
    delim: u8,
    buf: Option<&mut Vec<u8>>,
) -> (bool, usize) {
    let (done, used) = memchr(delim, available)
        .map_or((false, available.len()), |i| (true, i + 1));
    if let Some(buf) = buf {
        buf.extend_from_slice(available.get(..used).unwrap_or_default());
    }
    (done, used)
}
