//! [`UdpSocket`].

use core::fmt;

use super::{
    macros::socket_methods, socket_addr::each_addr, tcp::debug_socket,
};
use crate::{
    io,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs},
    sys::net::Socket,
};

/// A UDP socket.
///
/// After [binding](Self::bind) it, data can be sent to and received from any
/// address, or only from the peer it is [connected](Self::connect) to.
/// Receiving blocks until a datagram arrives, up to the read timeout if one
/// is set, unless the socket is in nonblocking mode.
pub struct UdpSocket {
    pub(crate) inner: Socket,
}

impl UdpSocket {
    /// Creates a UDP socket bound to the first address `addr` yields that
    /// succeeds.
    ///
    /// # Errors
    ///
    /// Fails if resolving `addr` fails or yields nothing, or with the error of
    /// the last attempt, such as [`io::ErrorKind::AddrInUse`].
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<Self> {
        each_addr(addr, Socket::bind_datagram).map(|inner| Self { inner })
    }

    /// Receives a single datagram, returning the number of bytes read and the
    /// origin. Bytes that do not fit in `buf` may be discarded.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.inner.recv_datagram_from(buf)
    }

    /// Like [`recv_from`](Self::recv_from), but leaves the datagram in the
    /// queue (`MSG_PEEK`).
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn peek_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.inner.peek_from(buf)
    }

    /// Sends data to the first address `addr` yields, returning the number of
    /// bytes written.
    ///
    /// # Errors
    ///
    /// Fails with [`io::ErrorKind::InvalidInput`] if `addr` yields no address,
    /// or with the error of resolving it or of sending.
    pub fn send_to<A: ToSocketAddrs>(
        &self,
        buf: &[u8],
        addr: A,
    ) -> io::Result<usize> {
        let Some(addr) = addr.to_socket_addrs()?.next() else {
            return Err(io::const_error!(
                io::ErrorKind::InvalidInput,
                "no addresses to send data to",
            ));
        };
        self.inner.send_to(buf, &addr)
    }

    /// Returns the socket address of the remote peer this socket was
    /// connected to.
    ///
    /// # Errors
    ///
    /// Fails with [`io::ErrorKind::NotConnected`] if not connected.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.peer_addr()
    }

    /// Returns the socket address that this socket was created from.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.socket_addr()
    }

    /// Sets the value of the `SO_BROADCAST` option, which allows sending to a
    /// broadcast address.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn set_broadcast(&self, broadcast: bool) -> io::Result<()> {
        self.inner.set_broadcast(broadcast)
    }

    /// Gets the value of the `SO_BROADCAST` option for this socket.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn broadcast(&self) -> io::Result<bool> {
        self.inner.broadcast()
    }

    /// Sets the value of the `IP_MULTICAST_LOOP` option, which loops
    /// multicast packets back to the local socket.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn set_multicast_loop_v4(
        &self,
        multicast_loop_v4: bool,
    ) -> io::Result<()> {
        self.inner.set_multicast_loop_v4(multicast_loop_v4)
    }

    /// Gets the value of the `IP_MULTICAST_LOOP` option for this socket.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn multicast_loop_v4(&self) -> io::Result<bool> {
        self.inner.multicast_loop_v4()
    }

    /// Sets the value of the `IP_MULTICAST_TTL` option: the time-to-live of
    /// outgoing multicast packets, 1 by default.
    ///
    /// # Errors
    ///
    /// Fails with [`io::ErrorKind::InvalidInput`] for a value above 255,
    /// which std checks before asking the OS, and otherwise with the error
    /// the OS reports.
    pub fn set_multicast_ttl_v4(
        &self,
        multicast_ttl_v4: u32,
    ) -> io::Result<()> {
        if multicast_ttl_v4 > u32::from(u8::MAX) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        self.inner.set_multicast_ttl_v4(multicast_ttl_v4)
    }

    /// Gets the value of the `IP_MULTICAST_TTL` option for this socket.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn multicast_ttl_v4(&self) -> io::Result<u32> {
        self.inner.multicast_ttl_v4()
    }

    /// Sets the value of the `IPV6_MULTICAST_LOOP` option, which lets this
    /// socket see the multicast packets it sends.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn set_multicast_loop_v6(
        &self,
        multicast_loop_v6: bool,
    ) -> io::Result<()> {
        self.inner.set_multicast_loop_v6(multicast_loop_v6)
    }

    /// Gets the value of the `IPV6_MULTICAST_LOOP` option for this socket.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn multicast_loop_v6(&self) -> io::Result<bool> {
        self.inner.multicast_loop_v6()
    }

    /// Executes an operation of the `IP_ADD_MEMBERSHIP` type: joins the
    /// multicast group `multiaddr` on the local `interface`, which the system
    /// chooses if it is [`UNSPECIFIED`](Ipv4Addr::UNSPECIFIED).
    ///
    /// # Errors
    ///
    /// Fails with [`io::ErrorKind::InvalidInput`] for a non-multicast address.
    pub fn join_multicast_v4(
        &self,
        multiaddr: &Ipv4Addr,
        interface: &Ipv4Addr,
    ) -> io::Result<()> {
        self.inner.join_multicast_v4(*multiaddr, *interface)
    }

    /// Executes an operation of the `IPV6_ADD_MEMBERSHIP` type: joins the
    /// multicast group `multiaddr` on the interface with index `interface`,
    /// or on any interface for 0.
    ///
    /// # Errors
    ///
    /// Fails with [`io::ErrorKind::InvalidInput`] for a non-multicast address.
    pub fn join_multicast_v6(
        &self,
        multiaddr: &Ipv6Addr,
        interface: u32,
    ) -> io::Result<()> {
        self.inner.join_multicast_v6(multiaddr, interface)
    }

    /// Executes an operation of the `IP_DROP_MEMBERSHIP` type.
    ///
    /// # Errors
    ///
    /// Returns the OS error, such as for a group the socket has not joined.
    pub fn leave_multicast_v4(
        &self,
        multiaddr: &Ipv4Addr,
        interface: &Ipv4Addr,
    ) -> io::Result<()> {
        self.inner.leave_multicast_v4(*multiaddr, *interface)
    }

    /// Executes an operation of the `IPV6_DROP_MEMBERSHIP` type.
    ///
    /// # Errors
    ///
    /// Returns the OS error, such as for a group the socket has not joined.
    pub fn leave_multicast_v6(
        &self,
        multiaddr: &Ipv6Addr,
        interface: u32,
    ) -> io::Result<()> {
        self.inner.leave_multicast_v6(multiaddr, interface)
    }

    /// Connects this socket to the first address `addr` yields that the OS
    /// accepts. [`send`](Self::send) and [`recv`](Self::recv) then use that
    /// peer, and datagrams from other addresses are filtered out.
    ///
    /// # Errors
    ///
    /// Fails if resolving `addr` fails or yields nothing, or with the error of
    /// the last attempt.
    pub fn connect<A: ToSocketAddrs>(&self, addr: A) -> io::Result<()> {
        each_addr(addr, |addr| self.inner.connect(addr))
    }

    /// Sends data to the connected peer, returning the number of bytes
    /// written.
    ///
    /// # Errors
    ///
    /// Fails if the socket is not connected, or with another OS error.
    pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
        self.inner.send_datagram(buf)
    }

    /// Receives a single datagram from the connected peer, returning the
    /// number of bytes read. Bytes that do not fit in `buf` may be discarded.
    ///
    /// # Errors
    ///
    /// Fails if the socket is not connected, or with another OS error.
    pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }

    /// Like [`recv`](Self::recv), but leaves the datagram in the queue
    /// (`MSG_PEEK`).
    ///
    /// # Errors
    ///
    /// Fails if the socket is not connected, or with another OS error.
    pub fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.peek(buf)
    }

    socket_methods!(common timeouts ttl);
}

impl fmt::Debug for UdpSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        debug_socket(f, "UdpSocket", &self.inner, false)
    }
}
