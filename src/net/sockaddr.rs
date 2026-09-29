//! The C form of IP socket addresses, which the backends share.
//!
//! `sockaddr_in` and `sockaddr_in6` have the same layout on every supported
//! system but for their first two bytes: a 16-bit family on Linux and
//! Windows, a length and an 8-bit family on macOS, which the backend's
//! `sockaddr_head` and `sockaddr_family` read and write. These byte mirrors
//! spell the layout out so the conversions need no `unsafe`; each backend
//! checks at compile time that they match its C types.

use core::ptr;

use crate::{
    io,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    sys::net::{AF_INET, AF_INET6, sockaddr_family, sockaddr_head},
};

/// `sockaddr_in`, with the port and the address as the bytes they are in
/// network order.
#[repr(C, align(4))]
#[derive(Clone, Copy)]
pub(crate) struct V4 {
    pub(crate) head: [u8; 2],
    pub(crate) port: [u8; 2],
    pub(crate) addr: [u8; 4],
    pub(crate) zero: [u8; 8],
}

/// `sockaddr_in6`, with the port and the address as the bytes they are in
/// network order. The flow info and scope id pass through as in std.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct V6 {
    pub(crate) head: [u8; 2],
    pub(crate) port: [u8; 2],
    pub(crate) flowinfo: u32,
    pub(crate) addr: [u8; 16],
    pub(crate) scope_id: u32,
}

/// Either form, as small as the larger one. Rust never reads it back, so
/// the bytes after a `V4` may stay uninitialized: the OS reads only the
/// first `SockAddr::len` bytes.
#[repr(C)]
union Repr {
    v4: V4,
    v6: V6,
}

/// A socket address in the form `bind`, `connect` and `sendto` read.
pub(crate) struct SockAddr {
    repr: Repr,
    len: u8,
}

/// The length of a `V4` or a `V6`, which fits a `u8`.
#[allow(clippy::cast_possible_truncation, reason = "16 and 28 bytes")]
const fn len_of<T>() -> u8 {
    size_of::<T>() as u8
}

impl SockAddr {
    pub(crate) const fn new(addr: &SocketAddr) -> Self {
        match addr {
            SocketAddr::V4(a) => Self {
                repr: Repr {
                    v4: V4 {
                        head: sockaddr_head(AF_INET, len_of::<V4>()),
                        port: a.port().to_be_bytes(),
                        addr: a.ip().octets(),
                        zero: [0; 8],
                    },
                },
                len: len_of::<V4>(),
            },
            SocketAddr::V6(a) => Self {
                repr: Repr {
                    v6: V6 {
                        head: sockaddr_head(AF_INET6, len_of::<V6>()),
                        port: a.port().to_be_bytes(),
                        flowinfo: a.flowinfo(),
                        addr: a.ip().octets(),
                        scope_id: a.scope_id(),
                    },
                },
                len: len_of::<V6>(),
            },
        }
    }

    /// Points to the address, as the C type `T` the OS call takes. The
    /// first [`len`](Self::len) bytes are initialized and stay valid while
    /// `self` does.
    pub(crate) const fn as_ptr<T>(&self) -> *const T {
        ptr::from_ref(&self.repr).cast()
    }

    /// The length of the address in bytes: 16 for IPv4, 28 for IPv6.
    pub(crate) const fn len(&self) -> u8 {
        self.len
    }
}

/// Room for any socket address the OS returns, sized like
/// `sockaddr_storage`, but for OpenBSD's twice larger one, and at least as
/// aligned. It starts out zeroed and every
/// byte pattern is valid, so the OS may write any bytes to it.
#[repr(C, align(8))]
pub(crate) struct Storage([u8; 128]);

impl Storage {
    /// The size to pass along with [`as_mut_ptr`](Self::as_mut_ptr).
    #[allow(clippy::cast_possible_truncation, reason = "128 bytes")]
    pub(crate) const LEN: u8 = size_of::<Self>() as u8;

    pub(crate) const fn new() -> Self {
        Self([0; 128])
    }

    /// Points to the storage, as the C type `T` the OS call takes; it is
    /// valid for writes of [`LEN`](Self::LEN) bytes.
    pub(crate) const fn as_mut_ptr<T>(&mut self) -> *mut T {
        self.0.as_mut_ptr().cast()
    }

    /// Converts the address the OS stored and reported as `len` bytes long,
    /// failing with `InvalidInput`, as std does, for another family or a
    /// short address. A `len` beyond the storage, as for an address the OS
    /// truncated, is never that of an IP address.
    pub(crate) fn to_socket_addr(&self, len: usize) -> io::Result<SocketAddr> {
        let bytes = self.0.get(..len).unwrap_or(&self.0);
        from_bytes(bytes).ok_or(INVALID_ADDRESS)
    }
}

/// The error for an address of another family, as std reports it.
const INVALID_ADDRESS: io::Error =
    io::const_error!(io::ErrorKind::InvalidInput, "invalid argument");

/// Converts the C socket address in `bytes`, all the bytes the OS gave for
/// it, if it is an IPv4 or IPv6 address as long as its family requires.
pub(crate) fn from_bytes(bytes: &[u8]) -> Option<SocketAddr> {
    let (&head, _) = bytes.split_first_chunk::<2>()?;
    match sockaddr_family(head) {
        AF_INET => {
            let &[_, _, p0, p1, a, b, c, d, ..] = bytes.first_chunk::<16>()?;
            let ip = Ipv4Addr::new(a, b, c, d);
            let port = u16::from_be_bytes([p0, p1]);
            Some(SocketAddr::V4(SocketAddrV4::new(ip, port)))
        }
        AF_INET6 => {
            let &[_, _, p0, p1, f0, f1, f2, f3, ref ip @ .., s0, s1, s2, s3] =
                bytes.first_chunk::<28>()?;
            Some(SocketAddr::V6(SocketAddrV6::new(
                Ipv6Addr::from(*ip),
                u16::from_be_bytes([p0, p1]),
                u32::from_ne_bytes([f0, f1, f2, f3]),
                u32::from_ne_bytes([s0, s1, s2, s3]),
            )))
        }
        _ => None,
    }
}
