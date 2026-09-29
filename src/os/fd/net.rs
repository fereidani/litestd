//! The descriptor traits for the sockets of [`net`](crate::net) and, with
//! `path`, of `os::unix::net`.

use super::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
#[cfg(feature = "path")]
use crate::os::unix::net::{UnixDatagram, UnixListener, UnixStream};
use crate::{
    net::{TcpListener, TcpStream, UdpSocket},
    sys::net::Socket,
};

macro_rules! impl_fd_traits {
    ($($t:ident)*) => {$(
        impl AsFd for $t {
            #[inline]
            fn as_fd(&self) -> BorrowedFd<'_> {
                self.inner.as_inner().as_fd()
            }
        }

        impl From<$t> for OwnedFd {
            /// Takes ownership of the socket's file descriptor.
            #[inline]
            fn from(socket: $t) -> Self {
                socket.inner.into_inner()
            }
        }

        impl From<OwnedFd> for $t {
            /// Takes ownership of the socket `fd` refers to.
            #[inline]
            fn from(fd: OwnedFd) -> Self {
                Self {
                    inner: Socket::from_inner(fd),
                }
            }
        }

        impl AsRawFd for $t {
            #[inline]
            fn as_raw_fd(&self) -> RawFd {
                self.inner.as_inner().as_raw_fd()
            }
        }

        impl FromRawFd for $t {
            #[inline]
            unsafe fn from_raw_fd(fd: RawFd) -> Self {
                // SAFETY: the caller hands over an open socket that nothing
                // else owns.
                Self::from(unsafe { OwnedFd::from_raw_fd(fd) })
            }
        }

        impl IntoRawFd for $t {
            #[inline]
            fn into_raw_fd(self) -> RawFd {
                self.inner.into_inner().into_raw_fd()
            }
        }
    )*};
}

impl_fd_traits! { TcpStream TcpListener UdpSocket }
#[cfg(feature = "path")]
impl_fd_traits! { UnixDatagram UnixListener UnixStream }
