//! [`UnixStream`].

use super::{SocketAddr, debug_socket};
use crate::{
    fmt, io,
    net::macros::{impl_stream_io, socket_methods},
    path::Path,
    sys::net::{Socket, UnixType},
};

/// A Unix stream socket.
///
/// Reads and writes block, up to the timeout if one is set, unless the
/// socket is in nonblocking mode. Writes pass `MSG_NOSIGNAL`, so writing to
/// a closed connection fails with [`io::ErrorKind::BrokenPipe`] instead of
/// raising `SIGPIPE`.
pub struct UnixStream {
    pub(crate) inner: Socket,
}

impl fmt::Debug for UnixStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        debug_socket(f, "UnixStream", &self.inner, true)
    }
}

impl UnixStream {
    /// Connects to the socket named by `path`, blocking until the connection
    /// is established or fails.
    ///
    /// # Errors
    ///
    /// Fails with [`io::ErrorKind::InvalidInput`] if `path` is too long or has
    /// a NUL byte, or with the OS error, such as [`io::ErrorKind::NotFound`].
    pub fn connect<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        Self::connect_addr(&SocketAddr::from_path(path.as_ref())?)
    }

    /// Connects to the socket specified by [`address`](SocketAddr), blocking
    /// until the connection is established or fails.
    ///
    /// # Errors
    ///
    /// Returns the OS error, such as [`io::ErrorKind::ConnectionRefused`].
    pub fn connect_addr(socket_addr: &SocketAddr) -> io::Result<Self> {
        let socket = Socket::unix(UnixType::Stream)?;
        socket.connect_unix(&socket_addr.inner)?;
        Ok(Self { inner: socket })
    }

    /// Creates an unnamed pair of connected sockets.
    ///
    /// # Errors
    ///
    /// Fails if the process has no descriptors left.
    pub fn pair() -> io::Result<(Self, Self)> {
        let (a, b) = Socket::unix_pair(UnixType::Stream)?;
        Ok((Self { inner: a }, Self { inner: b }))
    }

    /// Returns the socket address of the local half of this connection.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner
            .unix_socket_addr()
            .map(|inner| SocketAddr { inner })
    }

    /// Returns the socket address of the remote half of this connection.
    ///
    /// # Errors
    ///
    /// Fails with [`io::ErrorKind::NotConnected`] if not connected.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner
            .unix_peer_addr()
            .map(|inner| SocketAddr { inner })
    }

    socket_methods!(common timeouts shutdown);
}

impl_stream_io!(UnixStream, &UnixStream);
