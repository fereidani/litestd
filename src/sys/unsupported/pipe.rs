//! Pipes where the platform has none: creating one fails, as in std, so no
//! pipe exists to read or write.

use core::{convert::Infallible, fmt};

use alloc_crate::{string::String, vec::Vec};

use super::UNSUPPORTED;
use crate::io::{self, IoSlice, IoSliceMut};

/// An end of a pipe, of which there are none.
pub(crate) struct Pipe(Infallible);

/// Fails: the platform has no pipes.
pub(crate) fn pipe() -> io::Result<(Pipe, Pipe)> {
    Err(UNSUPPORTED)
}

unreachable_methods! {
    Pipe;
    fn try_clone(&self) -> io::Result<Self>;
    fn read(&self, buf: &mut [u8]) -> io::Result<usize>;
    fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize>;
    fn read_to_end(&self, buf: &mut Vec<u8>) -> io::Result<usize>;
    fn read_to_string(&self, buf: &mut String) -> io::Result<usize>;
    fn write(&self, buf: &[u8]) -> io::Result<usize>;
    fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize>;
}

impl fmt::Debug for Pipe {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {}
    }
}
