//! Owned and borrowed OS sockets, and raw ones.
//!
//! Unlike std, `Option<OwnedSocket>` and `Option<BorrowedSocket<'_>>` are
//! twice the size of a `RawSocket`: stable Rust cannot declare
//! `INVALID_SOCKET` as a niche.

use core::{fmt, marker::PhantomData, mem::ManuallyDrop};

use alloc_crate::{boxed::Box, rc::Rc, sync::Arc};

use crate::{io, net, os::windows::raw, sys};

/// Raw `SOCKET`s.
pub type RawSocket = raw::SOCKET;

/// `INVALID_SOCKET`, the one value that no socket has.
const INVALID: RawSocket = RawSocket::MAX;

/// Extracts raw sockets.
pub trait AsRawSocket {
    /// Extracts the raw socket, without passing ownership.
    fn as_raw_socket(&self) -> RawSocket;
}

/// Creates I/O objects from raw sockets.
pub trait FromRawSocket {
    /// Constructs a new I/O object that takes ownership of `sock`.
    ///
    /// # Safety
    ///
    /// `sock` must be open, not owned or borrowed by anything else, and freed
    /// by `closesocket`.
    unsafe fn from_raw_socket(sock: RawSocket) -> Self;
}

/// Consumes an object, handing over ownership of its raw `SOCKET`.
pub trait IntoRawSocket {
    /// Consumes this object, returning the raw socket for the caller to close.
    #[must_use = "losing the raw socket may leak resources"]
    fn into_raw_socket(self) -> RawSocket;
}

/// A borrowed socket, tied to the lifetime of its owner. It is
/// `repr(transparent)` over a raw socket and never `INVALID_SOCKET`.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct BorrowedSocket<'socket> {
    socket: RawSocket,
    _phantom: PhantomData<&'socket OwnedSocket>,
}

/// An owned socket, closed on drop, ignoring errors as std does. It is
/// `repr(transparent)` over a raw socket and never `INVALID_SOCKET`.
#[repr(transparent)]
pub struct OwnedSocket {
    socket: RawSocket,
}

impl BorrowedSocket<'_> {
    /// Returns a `BorrowedSocket` holding the given raw socket.
    ///
    /// # Safety
    ///
    /// `socket` must not be `INVALID_SOCKET` and must stay open for the
    /// lifetime of the result.
    #[inline]
    #[allow(clippy::must_use_candidate, reason = "not `#[must_use]` in std")]
    pub const unsafe fn borrow_raw(socket: RawSocket) -> Self {
        debug_assert!(socket != INVALID, "socket != -1");
        Self {
            socket,
            _phantom: PhantomData,
        }
    }

    /// Creates a new `OwnedSocket` for the same socket. Like every socket
    /// litestd creates, child processes do not inherit it.
    ///
    /// # Errors
    ///
    /// Returns the error of `WSADuplicateSocketW` or of creating the
    /// duplicate.
    pub fn try_clone_to_owned(&self) -> io::Result<OwnedSocket> {
        sys::net::duplicate(*self)
    }
}

impl OwnedSocket {
    /// Creates a new `OwnedSocket` for the same socket.
    ///
    /// # Errors
    ///
    /// As for [`BorrowedSocket::try_clone_to_owned`].
    pub fn try_clone(&self) -> io::Result<Self> {
        self.as_socket().try_clone_to_owned()
    }
}

impl AsRawSocket for BorrowedSocket<'_> {
    #[inline]
    fn as_raw_socket(&self) -> RawSocket {
        self.socket
    }
}

impl AsRawSocket for OwnedSocket {
    #[inline]
    fn as_raw_socket(&self) -> RawSocket {
        self.socket
    }
}

impl IntoRawSocket for OwnedSocket {
    #[inline]
    fn into_raw_socket(self) -> RawSocket {
        ManuallyDrop::new(self).socket
    }
}

impl FromRawSocket for OwnedSocket {
    #[inline]
    unsafe fn from_raw_socket(socket: RawSocket) -> Self {
        debug_assert!(socket != INVALID, "socket != -1");
        Self { socket }
    }
}

impl Drop for OwnedSocket {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: `self` owns the socket, and nothing uses it after the drop.
        unsafe { sys::net::close(self.socket) };
    }
}

impl fmt::Debug for BorrowedSocket<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BorrowedSocket")
            .field("socket", &self.socket)
            .finish()
    }
}

impl fmt::Debug for OwnedSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedSocket")
            .field("socket", &self.socket)
            .finish()
    }
}

/// A trait to borrow the socket from an underlying object.
pub trait AsSocket {
    /// Borrows the socket.
    fn as_socket(&self) -> BorrowedSocket<'_>;
}

forward_borrow!(AsSocket, as_socket, BorrowedSocket, []);

impl AsSocket for BorrowedSocket<'_> {
    #[inline]
    fn as_socket(&self) -> BorrowedSocket<'_> {
        *self
    }
}

impl AsSocket for OwnedSocket {
    #[inline]
    fn as_socket(&self) -> BorrowedSocket<'_> {
        // SAFETY: `self` holds an open socket other than `INVALID_SOCKET` and
        // closes it only on drop, which the borrow cannot outlive.
        unsafe { BorrowedSocket::borrow_raw(self.socket) }
    }
}

/// Implements the socket traits of std for a socket type of `net`.
macro_rules! impl_net_socket {
    ($($ty:ident)*) => {$(
        impl AsRawSocket for net::$ty {
            #[inline]
            fn as_raw_socket(&self) -> RawSocket {
                self.inner.as_inner().as_raw_socket()
            }
        }

        impl FromRawSocket for net::$ty {
            #[inline]
            unsafe fn from_raw_socket(sock: RawSocket) -> Self {
                // SAFETY: the caller hands over an owned socket, as
                // `OwnedSocket` requires.
                Self::from(unsafe { OwnedSocket::from_raw_socket(sock) })
            }
        }

        impl IntoRawSocket for net::$ty {
            #[inline]
            fn into_raw_socket(self) -> RawSocket {
                self.inner.into_inner().into_raw_socket()
            }
        }

        impl AsSocket for net::$ty {
            #[inline]
            fn as_socket(&self) -> BorrowedSocket<'_> {
                self.inner.as_inner().as_socket()
            }
        }

        impl From<net::$ty> for OwnedSocket {
            #[doc = concat!(
                "Takes ownership of a [`", stringify!($ty), "`](net::",
                stringify!($ty), ")'s socket.",
            )]
            #[inline]
            fn from(socket: net::$ty) -> Self {
                socket.inner.into_inner()
            }
        }

        impl From<OwnedSocket> for net::$ty {
            #[inline]
            fn from(owned: OwnedSocket) -> Self {
                Self {
                    inner: sys::net::Socket::from_inner(owned),
                }
            }
        }
    )*};
}

impl_net_socket! { TcpStream TcpListener UdpSocket }
