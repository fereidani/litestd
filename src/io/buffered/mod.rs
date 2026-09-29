//! Buffering wrappers for I/O traits: [`BufReader`], [`BufWriter`] and
//! [`LineWriter`].

mod bufreader;
mod bufwriter;
mod linewriter;

use core::{error, fmt};

pub use self::{
    bufreader::BufReader,
    bufwriter::{BufWriter, WriterPanicked},
    linewriter::LineWriter,
};
use super::Error;

/// The error of [`BufWriter::into_inner`]: the error that happened while
/// writing out the buffer, and the buffered writer, for recovery.
#[derive(Debug)]
pub struct IntoInnerError<W>(pub(super) W, pub(super) Error);

impl<W> IntoInnerError<W> {
    /// Returns the error which caused the call to [`BufWriter::into_inner`]
    /// to fail.
    #[allow(clippy::missing_const_for_fn, reason = "not `const` in std")]
    pub fn error(&self) -> &Error {
        &self.1
    }

    /// Returns the buffered writer instance which generated the error.
    pub fn into_inner(self) -> W {
        self.0
    }

    /// Consumes the [`IntoInnerError`] and returns the error.
    pub fn into_error(self) -> Error {
        self.1
    }

    /// Consumes the [`IntoInnerError`] and returns the error and the writer.
    pub fn into_parts(self) -> (Error, W) {
        (self.1, self.0)
    }
}

impl<W> From<IntoInnerError<W>> for Error {
    fn from(iie: IntoInnerError<W>) -> Self {
        iie.1
    }
}

impl<W: Send + fmt::Debug> error::Error for IntoInnerError<W> {}

impl<W> fmt::Display for IntoInnerError<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.1, f)
    }
}
