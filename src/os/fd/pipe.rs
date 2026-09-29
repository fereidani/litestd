//! The descriptor traits for the ends of [`io::pipe`](crate::io::pipe).

use super::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use crate::{
    io::{PipeReader, PipeWriter},
    sys::pipe::Pipe,
};

macro_rules! impl_fd_traits {
    ($($t:ident)*) => {$(
        impl AsFd for $t {
            #[inline]
            fn as_fd(&self) -> BorrowedFd<'_> {
                self.0.as_fd()
            }
        }

        impl From<$t> for OwnedFd {
            /// Takes ownership of the pipe's file descriptor.
            #[inline]
            fn from(pipe: $t) -> Self {
                pipe.0.into_fd()
            }
        }

        impl From<OwnedFd> for $t {
            /// Takes ownership of `fd`, which should be the matching end of
            /// a pipe.
            #[inline]
            fn from(fd: OwnedFd) -> Self {
                Self(Pipe::from(fd))
            }
        }

        impl AsRawFd for $t {
            #[inline]
            fn as_raw_fd(&self) -> RawFd {
                self.0.raw()
            }
        }

        impl FromRawFd for $t {
            #[inline]
            unsafe fn from_raw_fd(fd: RawFd) -> Self {
                // SAFETY: the caller hands over an open descriptor that
                // nothing else owns.
                Self::from(unsafe { OwnedFd::from_raw_fd(fd) })
            }
        }

        impl IntoRawFd for $t {
            #[inline]
            fn into_raw_fd(self) -> RawFd {
                self.0.into_fd().into_raw_fd()
            }
        }
    )*};
}

impl_fd_traits! { PipeReader PipeWriter }
