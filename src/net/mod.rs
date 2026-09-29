//! Networking primitives for TCP/UDP communication.
//!
//! The address types come from `core::net` and, like [`Shutdown`], are
//! always available; the sockets and [`ToSocketAddrs`] need the `net`
//! feature. Child processes do not inherit sockets: they are opened with
//! `CLOEXEC` on Linux, marked with it right after on macOS, as in std, and
//! opened without `HANDLE_FLAG_INHERIT` on Windows.

pub use core::net::*;

#[cfg(feature = "net")]
pub use self::{
    socket_addr::ToSocketAddrs,
    tcp::{Incoming, TcpListener, TcpStream},
    udp::UdpSocket,
};

// Exported by `os::linux::net` and `os::android::net`.
#[cfg(all(feature = "net", any(target_os = "linux", target_os = "android")))]
pub(crate) mod linux_ext;
#[cfg(feature = "net")]
pub(crate) mod macros;
// The C form of addresses, for the backends that have sockets.
#[cfg(all(feature = "net", any(unix, windows)))]
pub(crate) mod sockaddr;
#[cfg(feature = "net")]
mod socket_addr;
#[cfg(feature = "net")]
mod tcp;
#[cfg(feature = "net")]
mod udp;

/// Possible values which can be passed to the [`TcpStream::shutdown`]
/// method.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Shutdown {
    /// Shuts down reading: blocked and future reads return `Ok(0)`.
    Read,
    /// Shuts down writing: blocked and future writes return an error.
    Write,
    /// Shuts down both reading and writing.
    Both,
}

/// The error for a zero timeout, as std reports it, from every backend.
#[cfg(all(feature = "net", any(unix, windows)))]
pub(crate) const ZERO_TIMEOUT: crate::io::Error = crate::io::const_error!(
    crate::io::ErrorKind::InvalidInput,
    "cannot set a 0 duration timeout",
);

/// The error of `TcpStream::connect_timeout` on timeout, as std reports it.
#[cfg(all(feature = "net", any(unix, windows)))]
pub(crate) const CONNECT_TIMED_OUT: crate::io::Error = crate::io::const_error!(
    crate::io::ErrorKind::TimedOut,
    "connection timed out",
);
