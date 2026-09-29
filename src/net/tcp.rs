//! [`TcpStream`], [`TcpListener`] and [`Incoming`].

use core::{fmt, iter::FusedIterator, time::Duration};

use super::{
    macros::{impl_stream_io, socket_methods},
    socket_addr::each_addr,
};
use crate::{
    io,
    net::{SocketAddr, ToSocketAddrs},
    sys::net::Socket,
};

/// A TCP stream between a local and a remote socket.
///
/// The connection is closed when the value is dropped. Reads, writes and
/// [`peek`](Self::peek) block, up to the timeout if one is set, unless the
/// stream is in nonblocking mode. On Unix, writes pass `MSG_NOSIGNAL`, so
/// writing to a closed connection fails with [`io::ErrorKind::BrokenPipe`]
/// instead of raising `SIGPIPE`.
///
/// # Examples
///
/// ```no_run
/// use litestd::{io::prelude::*, net::TcpStream};
///
/// fn main() -> litestd::io::Result<()> {
///     let mut stream = TcpStream::connect("127.0.0.1:34254")?;
///     stream.write(&[1])?;
///     stream.read(&mut [0; 128])?;
///     Ok(())
/// }
/// ```
pub struct TcpStream {
    pub(crate) inner: Socket,
}

/// A TCP socket server, listening for connections.
///
/// The socket is closed when the value is dropped.
pub struct TcpListener {
    pub(crate) inner: Socket,
}

/// An iterator that infinitely [`accept`](TcpListener::accept)s connections
/// on a [`TcpListener`], created by [`TcpListener::incoming`].
#[must_use = "iterators are lazy and do nothing unless consumed"]
#[derive(Debug)]
pub struct Incoming<'a> {
    listener: &'a TcpListener,
}

/// Formats a socket as std does: `Name { addr: .., peer: .., fd: .. }`,
/// leaving out the addresses the OS does not report.
pub(super) fn debug_socket(
    f: &mut fmt::Formatter<'_>,
    name: &str,
    socket: &Socket,
    peer: bool,
) -> fmt::Result {
    let mut res = f.debug_struct(name);
    if let Ok(addr) = socket.socket_addr() {
        res.field("addr", &addr);
    }
    if peer {
        if let Ok(addr) = socket.peer_addr() {
            res.field("peer", &addr);
        }
    }
    let (raw_name, raw) = socket.raw_debug();
    res.field(raw_name, &raw).finish()
}

impl TcpStream {
    /// Opens a TCP connection to a remote host, trying each address `addr`
    /// yields until one connects. Blocks until then.
    ///
    /// # Errors
    ///
    /// Fails if resolving `addr` fails or yields nothing, or with the error of
    /// the last attempt, such as [`io::ErrorKind::ConnectionRefused`].
    pub fn connect<A: ToSocketAddrs>(addr: A) -> io::Result<Self> {
        each_addr(addr, Socket::connect_stream).map(|inner| Self { inner })
    }

    /// Opens a TCP connection to `addr`, blocking for at most `timeout`.
    ///
    /// # Errors
    ///
    /// Fails with [`io::ErrorKind::InvalidInput`] for a zero `timeout`, with
    /// [`io::ErrorKind::TimedOut`] once it passes, or with the OS error.
    pub fn connect_timeout(
        addr: &SocketAddr,
        timeout: Duration,
    ) -> io::Result<Self> {
        Socket::connect_stream_timeout(addr, timeout)
            .map(|inner| Self { inner })
    }

    /// Returns the socket address of the remote peer of this TCP connection.
    ///
    /// # Errors
    ///
    /// Fails with [`io::ErrorKind::NotConnected`] if not connected.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.peer_addr()
    }

    /// Returns the socket address of the local half of this TCP connection.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.socket_addr()
    }

    /// Receives data from the peer without removing it from the queue
    /// (`MSG_PEEK`), returning the number of bytes peeked.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.peek(buf)
    }

    /// Sets the value of the `TCP_NODELAY` option, which disables Nagle's
    /// algorithm.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        self.inner.set_nodelay(nodelay)
    }

    /// Gets the value of the `TCP_NODELAY` option on this socket.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn nodelay(&self) -> io::Result<bool> {
        self.inner.nodelay()
    }

    socket_methods!(common timeouts ttl shutdown);
}

impl_stream_io!(TcpStream, &TcpStream);

impl fmt::Debug for TcpStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        debug_socket(f, "TcpStream", &self.inner, true)
    }
}

impl TcpListener {
    /// Creates a new `TcpListener` bound to the first address `addr` yields
    /// that succeeds. Port 0 asks the OS to assign a port.
    ///
    /// # Errors
    ///
    /// Fails if resolving `addr` fails or yields nothing, or with the error of
    /// the last attempt, such as [`io::ErrorKind::AddrInUse`].
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<Self> {
        each_addr(addr, Socket::listen_stream).map(|inner| Self { inner })
    }

    /// Returns the local socket address of this listener.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.socket_addr()
    }

    /// Accepts a new incoming connection, blocking until one arrives, and
    /// returns the stream with the peer's address.
    ///
    /// # Errors
    ///
    /// Returns the OS error; `EINTR` is retried on Unix. Errors such as
    /// [`io::ErrorKind::ConnectionAborted`] leave the listener usable.
    pub fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        let (inner, addr) = self.inner.accept()?;
        Ok((TcpStream { inner }, addr))
    }

    /// Returns an iterator that calls [`accept`](Self::accept) in a loop,
    /// blocking like it, and never returns [`None`].
    #[allow(
        clippy::must_use_candidate,
        clippy::missing_const_for_fn,
        reason = "neither `#[must_use]` nor `const` in std"
    )]
    pub fn incoming(&self) -> Incoming<'_> {
        Incoming { listener: self }
    }

    socket_methods!(common ttl);
}

impl Iterator for Incoming<'_> {
    type Item = io::Result<TcpStream>;

    fn next(&mut self) -> Option<io::Result<TcpStream>> {
        Some(self.listener.accept().map(|p| p.0))
    }
}

impl FusedIterator for Incoming<'_> {}

impl fmt::Debug for TcpListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        debug_socket(f, "TcpListener", &self.inner, false)
    }
}
