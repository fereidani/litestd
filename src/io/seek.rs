//! The [`Seek`] trait and [`SeekFrom`].

use super::Result;

/// The `Seek` trait provides a cursor which can be moved within a stream of
/// bytes.
pub trait Seek {
    /// Seeks to an offset, in bytes, in a stream, and returns the new position
    /// from the start of the stream. Seeking beyond the end is allowed, with
    /// implementation-defined behavior.
    ///
    /// # Errors
    ///
    /// Seeking to a negative offset fails, and so can the I/O a seek involves.
    fn seek(&mut self, pos: SeekFrom) -> Result<u64>;

    /// Rewinds to the beginning of a stream, as `seek(SeekFrom::Start(0))`.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`seek`](Seek::seek).
    fn rewind(&mut self) -> Result<()> {
        self.seek(SeekFrom::Start(0))?;
        Ok(())
    }

    /// Returns the current seek position from the start of the stream, as
    /// `seek(SeekFrom::Current(0))` does.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`seek`](Seek::seek).
    #[allow(clippy::seek_from_current, reason = "this is stream_position")]
    fn stream_position(&mut self) -> Result<u64> {
        self.seek(SeekFrom::Current(0))
    }

    /// Seeks relative to the current position without returning it, which
    /// lets [`BufReader`](super::BufReader) seek within its buffer.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`seek`](Seek::seek).
    fn seek_relative(&mut self, offset: i64) -> Result<()> {
        self.seek(SeekFrom::Current(offset))?;
        Ok(())
    }
}

/// Enumeration of possible methods to seek within an I/O object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeekFrom {
    /// Sets the offset to the provided number of bytes.
    Start(u64),
    /// Sets the offset to the size of this object plus the specified number
    /// of bytes; seeking before byte 0 is an error.
    End(i64),
    /// Sets the offset to the current position plus the specified number of
    /// bytes; seeking before byte 0 is an error.
    Current(i64),
}
