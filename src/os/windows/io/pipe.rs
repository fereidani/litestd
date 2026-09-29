//! The handle traits for the ends of [`io::pipe`](crate::io::pipe).

use super::{
    AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, IntoRawHandle,
    OwnedHandle, RawHandle,
};
use crate::{
    io::{PipeReader, PipeWriter},
    sys::pipe::Pipe,
};

macro_rules! impl_handle_traits {
    ($($t:ident)*) => {$(
        impl AsHandle for $t {
            #[inline]
            fn as_handle(&self) -> BorrowedHandle<'_> {
                self.0.handle().as_handle()
            }
        }

        impl From<$t> for OwnedHandle {
            /// Takes ownership of the pipe's handle.
            #[inline]
            fn from(pipe: $t) -> Self {
                pipe.0.into_handle()
            }
        }

        impl From<OwnedHandle> for $t {
            /// Takes ownership of `handle`, which should be the matching end
            /// of a pipe.
            #[inline]
            fn from(handle: OwnedHandle) -> Self {
                Self(Pipe::from(handle))
            }
        }

        impl AsRawHandle for $t {
            #[inline]
            fn as_raw_handle(&self) -> RawHandle {
                self.0.handle().as_raw_handle()
            }
        }

        impl FromRawHandle for $t {
            #[inline]
            unsafe fn from_raw_handle(handle: RawHandle) -> Self {
                // SAFETY: the caller hands over an open handle that nothing
                // else owns.
                Self::from(unsafe { OwnedHandle::from_raw_handle(handle) })
            }
        }

        impl IntoRawHandle for $t {
            #[inline]
            fn into_raw_handle(self) -> RawHandle {
                self.0.into_handle().into_raw_handle()
            }
        }
    )*};
}

impl_handle_traits! { PipeReader PipeWriter }
