//! [`UnixListener`] and [`Incoming`].

use super::{SocketAddr, UnixStream, debug_socket};
use crate::{
    fmt, io,
    net::macros::socket_methods,
    path::Path,
    sys::net::{Socket, UnixType},
};

/// A structure representing a Unix domain socket server.
pub struct UnixListener {
    pub(crate) inner: Socket,
}

impl fmt::Debug for UnixListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        debug_socket(f, "UnixListener", &self.inner, false)
    }
}

impl UnixListener {
    /// Creates a new `UnixListener` bound to the specified socket.
    ///
    /// # Errors
    ///
    /// Fails with [`io::ErrorKind::InvalidInput`] if `path` is too long or has
    /// a NUL byte, or with [`io::ErrorKind::AddrInUse`] if the file exists.
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        Self::listen(&SocketAddr::from_path(path.as_ref())?, true)
    }

    /// Creates a new `UnixListener` bound to the specified
    /// [`socket address`](SocketAddr).
    ///
    /// # Errors
    ///
    /// Returns the OS error, such as [`io::ErrorKind::AddrInUse`].
    pub fn bind_addr(socket_addr: &SocketAddr) -> io::Result<Self> {
        Self::listen(socket_addr, false)
    }

    /// Binds a new socket to `socket_addr` and listens on it, with the
    /// backlog std gives `bind` if `by_path`, else `bind_addr`'s.
    fn listen(socket_addr: &SocketAddr, by_path: bool) -> io::Result<Self> {
        let socket = Socket::unix(UnixType::Stream)?;
        socket.bind_unix(&socket_addr.inner)?;
        socket.listen_unix(by_path)?;
        Ok(Self { inner: socket })
    }

    /// Accepts a new incoming connection, blocking until one arrives, and
    /// returns the stream with the peer's address.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports; `EINTR` is retried.
    pub fn accept(&self) -> io::Result<(UnixStream, SocketAddr)> {
        let (socket, inner) = self.inner.accept_unix()?;
        Ok((UnixStream { inner: socket }, SocketAddr { inner }))
    }

    /// Returns the local socket address of this listener.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner
            .unix_socket_addr()
            .map(|inner| SocketAddr { inner })
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

    socket_methods!(common);
}

#[allow(clippy::into_iter_without_iter, reason = "std's API, with `incoming`")]
impl<'a> IntoIterator for &'a UnixListener {
    type Item = io::Result<UnixStream>;
    type IntoIter = Incoming<'a>;

    fn into_iter(self) -> Incoming<'a> {
        self.incoming()
    }
}

/// An iterator over incoming connections to a [`UnixListener`].
///
/// It will never return [`None`].
#[derive(Debug)]
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct Incoming<'a> {
    listener: &'a UnixListener,
}

impl Iterator for Incoming<'_> {
    type Item = io::Result<UnixStream>;

    fn next(&mut self) -> Option<io::Result<UnixStream>> {
        Some(self.listener.accept().map(|s| s.0))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (usize::MAX, None)
    }
}
