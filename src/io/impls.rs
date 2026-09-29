//! The I/O trait implementations for references, boxes, byte slices,
//! `Vec<u8>` and `VecDeque<u8>`.

use core::{fmt, mem, str};

use alloc_crate::{
    boxed::Box, collections::VecDeque, string::String, vec::Vec,
};

use super::{
    BufRead, Error, IoSlice, IoSliceMut, Read, Result, Seek, SeekFrom, Write,
    append_to_string,
};

/// Copies as many bytes as fit from the front of `src` to the front of
/// `dst`, returning how many.
#[inline]
pub(crate) fn copy_prefix(dst: &mut [u8], src: &[u8]) -> usize {
    let n = dst.len().min(src.len());
    if let (Some(dst), Some(src)) = (dst.get_mut(..n), src.get(..n)) {
        // A call to `memcpy` costs more than copying a single byte.
        if let ([d], [s]) = (&mut *dst, src) {
            *d = *s;
        } else {
            dst.copy_from_slice(src);
        }
    }
    n
}

/// The count `n` as a length, at most `max`.
#[inline]
pub(crate) fn min_len(n: u64, max: usize) -> usize {
    usize::try_from(n).map_or(max, |n| n.min(max))
}

/// The total length of `bufs`, saturated: slices may alias, so the true
/// total can exceed `usize::MAX`.
pub(crate) fn total_len(bufs: &[IoSlice<'_>]) -> usize {
    bufs.iter()
        .fold(0, |total, b| total.saturating_add(b.len()))
}

/// Implements a trait for each given pointer to a type `T` that has it, by
/// forwarding every method, so that the overrides of `T` are kept.
macro_rules! forward {
    (Read for $($ptr:ty),+) => {$(
        impl<T: Read + ?Sized> Read for $ptr {
            #[inline]
            fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
                (**self).read(buf)
            }

            #[inline]
            fn read_vectored(
                &mut self,
                bufs: &mut [IoSliceMut<'_>],
            ) -> Result<usize> {
                (**self).read_vectored(bufs)
            }

            #[inline]
            fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
                (**self).read_to_end(buf)
            }

            #[inline]
            fn read_to_string(&mut self, buf: &mut String) -> Result<usize> {
                (**self).read_to_string(buf)
            }

            #[inline]
            fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
                (**self).read_exact(buf)
            }
        }
    )+};
    (BufRead for $($ptr:ty),+) => {$(
        impl<T: BufRead + ?Sized> BufRead for $ptr {
            #[inline]
            fn fill_buf(&mut self) -> Result<&[u8]> {
                (**self).fill_buf()
            }

            #[inline]
            fn consume(&mut self, amount: usize) {
                (**self).consume(amount);
            }

            #[inline]
            fn read_until(
                &mut self,
                byte: u8,
                buf: &mut Vec<u8>,
            ) -> Result<usize> {
                (**self).read_until(byte, buf)
            }

            #[inline]
            fn skip_until(&mut self, byte: u8) -> Result<usize> {
                (**self).skip_until(byte)
            }

            #[inline]
            fn read_line(&mut self, buf: &mut String) -> Result<usize> {
                (**self).read_line(buf)
            }
        }
    )+};
    (Write for $($ptr:ty),+) => {$(
        impl<T: Write + ?Sized> Write for $ptr {
            #[inline]
            fn write(&mut self, buf: &[u8]) -> Result<usize> {
                (**self).write(buf)
            }

            #[inline]
            fn write_vectored(
                &mut self,
                bufs: &[IoSlice<'_>],
            ) -> Result<usize> {
                (**self).write_vectored(bufs)
            }

            #[inline]
            fn flush(&mut self) -> Result<()> {
                (**self).flush()
            }

            #[inline]
            fn write_all(&mut self, buf: &[u8]) -> Result<()> {
                (**self).write_all(buf)
            }

            #[inline]
            fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> Result<()> {
                (**self).write_fmt(args)
            }
        }
    )+};
    (Seek for $($ptr:ty),+) => {$(
        impl<T: Seek + ?Sized> Seek for $ptr {
            #[inline]
            fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
                (**self).seek(pos)
            }

            #[inline]
            fn rewind(&mut self) -> Result<()> {
                (**self).rewind()
            }

            #[inline]
            fn stream_position(&mut self) -> Result<u64> {
                (**self).stream_position()
            }

            #[inline]
            fn seek_relative(&mut self, offset: i64) -> Result<()> {
                (**self).seek_relative(offset)
            }
        }
    )+};
}

forward!(Read for &mut T, Box<T>);
forward!(BufRead for &mut T, Box<T>);
forward!(Write for &mut T, Box<T>);
forward!(Seek for &mut T, Box<T>);

// In-memory buffers.

/// Reading from a byte slice advances it past the bytes read.
impl Read for &[u8] {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let n = copy_prefix(buf, self);
        *self = self.get(n..).unwrap_or_default();
        Ok(n)
    }

    #[inline]
    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> Result<usize> {
        let mut read = 0;
        for buf in bufs {
            read += self.read(buf)?;
            if self.is_empty() {
                break;
            }
        }
        Ok(read)
    }

    /// Fails without copying anything if the slice is too short, leaving it
    /// empty.
    #[inline]
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        let Some((head, tail)) = self.split_at_checked(buf.len()) else {
            *self = &[];
            return Err(Error::READ_EXACT_EOF);
        };
        copy_prefix(buf, head);
        *self = tail;
        Ok(())
    }

    #[inline]
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
        buf.try_reserve(self.len())?;
        buf.extend_from_slice(self);
        Ok(mem::take(self).len())
    }

    /// Fails without consuming anything if the slice is not valid UTF-8.
    #[inline]
    fn read_to_string(&mut self, buf: &mut String) -> Result<usize> {
        let text = str::from_utf8(self).map_err(|_| Error::INVALID_UTF8)?;
        buf.try_reserve(text.len())?;
        buf.push_str(text);
        Ok(mem::take(self).len())
    }
}

impl BufRead for &[u8] {
    #[inline]
    fn fill_buf(&mut self) -> Result<&[u8]> {
        Ok(*self)
    }

    #[inline]
    fn consume(&mut self, amount: usize) {
        *self = self.get(amount..).unwrap_or_default();
    }
}

/// Writing to a mutable byte slice overwrites its front and advances it
/// past the bytes written.
impl Write for &mut [u8] {
    #[inline]
    fn write(&mut self, data: &[u8]) -> Result<usize> {
        let n = copy_prefix(self, data);
        *self = mem::take(self).get_mut(n..).unwrap_or_default();
        Ok(n)
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> Result<usize> {
        let mut written = 0;
        for buf in bufs {
            written += self.write(buf)?;
            if self.is_empty() {
                break;
            }
        }
        Ok(written)
    }

    #[inline]
    fn write_all(&mut self, data: &[u8]) -> Result<()> {
        if self.write(data)? < data.len() {
            Err(Error::WRITE_ALL_EOF)
        } else {
            Ok(())
        }
    }

    #[inline]
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Writing to a vector appends to it.
impl Write for Vec<u8> {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.extend_from_slice(buf);
        Ok(buf.len())
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> Result<usize> {
        let len = total_len(bufs);
        self.reserve(len);
        for buf in bufs {
            self.extend_from_slice(buf);
        }
        Ok(len)
    }

    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        self.extend_from_slice(buf);
        Ok(())
    }

    #[inline]
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Reading from a deque removes bytes from its front.
impl Read for VecDeque<u8> {
    /// Reads from the front slice of the deque only.
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let n = copy_prefix(buf, self.as_slices().0);
        self.drain(..n.min(self.len()));
        Ok(n)
    }

    /// Fails without copying anything if the deque is too short, leaving it
    /// empty.
    #[inline]
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        if buf.len() > self.len() {
            self.clear();
            return Err(Error::READ_EXACT_EOF);
        }
        let (front, back) = self.as_slices();
        let n = copy_prefix(buf, front);
        if let Some(rest) = buf.get_mut(n..) {
            copy_prefix(rest, back);
        }
        self.drain(..buf.len().min(self.len()));
        Ok(())
    }

    #[inline]
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
        let len = self.len();
        buf.try_reserve(len)?;
        let (front, back) = self.as_slices();
        buf.extend_from_slice(front);
        buf.extend_from_slice(back);
        self.clear();
        Ok(len)
    }

    /// Consumes the whole deque even if it is not valid UTF-8.
    #[inline]
    fn read_to_string(&mut self, buf: &mut String) -> Result<usize> {
        // SAFETY: `read_to_end` above only appends to the vector.
        unsafe { append_to_string(buf, |bytes| self.read_to_end(bytes)) }
    }
}

impl BufRead for VecDeque<u8> {
    /// Returns the front slice of the deque.
    #[inline]
    fn fill_buf(&mut self) -> Result<&[u8]> {
        Ok(self.as_slices().0)
    }

    #[inline]
    fn consume(&mut self, amount: usize) {
        self.drain(..amount.min(self.len()));
    }
}

/// Writing to a deque appends to it.
impl Write for VecDeque<u8> {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.extend(buf);
        Ok(buf.len())
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> Result<usize> {
        let len = total_len(bufs);
        self.reserve(len);
        for buf in bufs {
            self.extend(&**buf);
        }
        Ok(len)
    }

    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        self.extend(buf);
        Ok(())
    }

    #[inline]
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}
