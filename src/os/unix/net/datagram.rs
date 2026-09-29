//! [`UnixDatagram`].

use super::{SocketAddr, debug_socket};
use crate::{
    fmt, io,
    net::macros::socket_methods,
    path::Path,
    sys::net::{Socket, UnixType},
};

/// A Unix datagram socket.
///
/// Receiving blocks until a datagram arrives, up to the read timeout if one
/// is set, unless the socket is in nonblocking mode.
pub struct UnixDatagram {
    pub(crate) inner: Socket,
}

impl fmt::Debug for UnixDatagram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        debug_socket(f, "UnixDatagram", &self.inner, true)
    }
}

impl UnixDatagram {
    /// Creates a Unix datagram socket bound to the given path.
    ///
    /// # Errors
    ///
    /// Fails with [`io::ErrorKind::InvalidInput`] if `path` is too long or has
    /// a NUL byte, or with [`io::ErrorKind::AddrInUse`] if the file exists.
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        Self::bind_addr(&SocketAddr::from_path(path.as_ref())?)
    }

    /// Creates a Unix datagram socket bound to an address.
    ///
    /// # Errors
    ///
    /// Returns the OS error, such as [`io::ErrorKind::AddrInUse`].
    pub fn bind_addr(socket_addr: &SocketAddr) -> io::Result<Self> {
        let socket = Self::unbound()?;
        socket.inner.bind_unix(&socket_addr.inner)?;
        Ok(socket)
    }

    /// Creates a Unix Datagram socket which is not bound to any address.
    ///
    /// # Errors
    ///
    /// Fails if the process has no descriptors left.
    pub fn unbound() -> io::Result<Self> {
        Socket::unix(UnixType::Datagram).map(|inner| Self { inner })
    }

    /// Creates an unnamed pair of connected sockets.
    ///
    /// # Errors
    ///
    /// Fails if the process has no descriptors left.
    pub fn pair() -> io::Result<(Self, Self)> {
        let (a, b) = Socket::unix_pair(UnixType::Datagram)?;
        Ok((Self { inner: a }, Self { inner: b }))
    }

    /// Connects the socket to the specified path address: [`send`](Self::send)
    /// sends to it, and [`recv`](Self::recv) and [`recv_from`](Self::recv_from)
    /// receive only from it.
    ///
    /// # Errors
    ///
    /// Fails with [`io::ErrorKind::InvalidInput`] for a path that is too long
    /// or contains a NUL byte, or with the OS error.
    pub fn connect<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        self.connect_addr(&SocketAddr::from_path(path.as_ref())?)
    }

    /// Connects the socket to an address.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn connect_addr(&self, socket_addr: &SocketAddr) -> io::Result<()> {
        self.inner.connect_unix(&socket_addr.inner)
    }

    /// Returns the address of this socket.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner
            .unix_socket_addr()
            .map(|inner| SocketAddr { inner })
    }

    /// Returns the address of this socket's peer.
    ///
    /// # Errors
    ///
    /// Fails with [`io::ErrorKind::NotConnected`] if not connected.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner
            .unix_peer_addr()
            .map(|inner| SocketAddr { inner })
    }

    /// Receives data from the socket, returning the number of bytes read and
    /// the sender's address.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        let (n, inner) = self.inner.recv_from_unix(buf)?;
        Ok((n, SocketAddr { inner }))
    }

    /// Receives data from the socket, returning the number of bytes read.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }

    /// Sends data on the socket to the specified address, returning the
    /// number of bytes written.
    ///
    /// # Errors
    ///
    /// Fails with [`io::ErrorKind::InvalidInput`] for a path that is too long
    /// or contains a NUL byte, or with the OS error.
    pub fn send_to<P: AsRef<Path>>(
        &self,
        buf: &[u8],
        path: P,
    ) -> io::Result<usize> {
        self.send_to_addr(buf, &SocketAddr::from_path(path.as_ref())?)
    }

    /// Sends data on the socket to the specified [`SocketAddr`], returning
    /// the number of bytes written.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn send_to_addr(
        &self,
        buf: &[u8],
        socket_addr: &SocketAddr,
    ) -> io::Result<usize> {
        self.inner.send_to_unix(buf, &socket_addr.inner)
    }

    /// Sends data on the socket to the socket's peer, returning the number of
    /// bytes written.
    ///
    /// # Errors
    ///
    /// Fails if the socket is not connected, or with another OS error.
    pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    socket_methods!(common timeouts shutdown);
}
