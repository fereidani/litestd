//! TCP and UDP sockets.

use core::{
    ffi::{c_int, c_short},
    mem::offset_of,
    ptr,
    time::Duration,
};

// macOS names the IPv6 multicast membership options after RFC 3493.
#[cfg(any(target_os = "linux", target_os = "android"))]
use libc::{IPV6_ADD_MEMBERSHIP, IPV6_DROP_MEMBERSHIP};
#[cfg(not(any(target_os = "linux", target_os = "android")))]
use libc::{
    IPV6_JOIN_GROUP as IPV6_ADD_MEMBERSHIP,
    IPV6_LEAVE_GROUP as IPV6_DROP_MEMBERSHIP,
};

use super::{
    super::{os::MAX_LEN, time::monotonic},
    CAddr, NameFn, Socket,
};
use crate::{
    io,
    net::{
        CONNECT_TIMED_OUT, Ipv4Addr, Ipv6Addr, SocketAddr, ZERO_TIMEOUT,
        sockaddr::{SockAddr, Storage, V4, V6},
    },
};

// The shared C forms of `net::sockaddr` must match the kernel's. Their
// first two bytes, `head`, end with the family: a 16-bit one on Linux, an
// 8-bit one after the length on macOS.
const _: () = {
    let family = size_of::<libc::sa_family_t>();
    assert!(size_of::<V4>() == size_of::<libc::sockaddr_in>());
    assert!(align_of::<V4>() == align_of::<libc::sockaddr_in>());
    assert!(offset_of!(V4, head) == 0);
    assert!(offset_of!(libc::sockaddr_in, sin_family) + family == 2);
    assert!(offset_of!(V4, port) == offset_of!(libc::sockaddr_in, sin_port));
    assert!(offset_of!(V4, addr) == offset_of!(libc::sockaddr_in, sin_addr));
    assert!(size_of::<V6>() == size_of::<libc::sockaddr_in6>());
    assert!(align_of::<V6>() == align_of::<libc::sockaddr_in6>());
    assert!(offset_of!(V6, head) == 0);
    assert!(offset_of!(libc::sockaddr_in6, sin6_family) + family == 2);
    assert!(offset_of!(V6, port) == offset_of!(libc::sockaddr_in6, sin6_port));
    assert!(
        offset_of!(V6, flowinfo)
            == offset_of!(libc::sockaddr_in6, sin6_flowinfo)
    );
    assert!(offset_of!(V6, addr) == offset_of!(libc::sockaddr_in6, sin6_addr));
    assert!(
        offset_of!(V6, scope_id)
            == offset_of!(libc::sockaddr_in6, sin6_scope_id)
    );
    // OpenBSD's `sockaddr_storage` has 256 bytes, far more than the IP
    // addresses this holds, whose length every call passes.
    #[cfg(not(target_os = "openbsd"))]
    assert!(size_of::<Storage>() == size_of::<libc::sockaddr_storage>());
    assert!(size_of::<Storage>() >= size_of::<libc::sockaddr_in6>());
    // Over-alignment is fine, as on i686: the OS only writes into our own
    // `Storage` and never hands one to us.
    assert!(align_of::<Storage>() >= align_of::<libc::sockaddr_storage>());
};

/// The backlog of a listening TCP socket, std's.
const BACKLOG: c_int = 128;

/// The error std reports when `poll` flags a failed connection whose
/// `SO_ERROR` is clear.
const NO_ERROR_AFTER_POLLHUP: io::Error = io::const_error!(
    io::ErrorKind::Uncategorized,
    "no error set after POLLHUP",
);

/// The family of `addr`, for `socket`.
const fn family(addr: &SocketAddr) -> c_int {
    match addr {
        SocketAddr::V4(_) => libc::AF_INET,
        SocketAddr::V6(_) => libc::AF_INET6,
    }
}

/// Converts the time left to wait into a `poll` timeout: whole milliseconds,
/// rounded up so that `poll` never returns before the deadline, and whether
/// the value had to be capped at `c_int::MAX`, about 24 days.
fn poll_ms(left: Duration) -> (c_int, bool) {
    let ms = left
        .as_secs()
        .saturating_mul(1000)
        .saturating_add(u64::from(left.subsec_nanos().div_ceil(1_000_000)));
    c_int::try_from(ms).map_or((c_int::MAX, true), |ms| (ms, false))
}

/// Fails with `EMSGSIZE`, as std does, for a datagram longer than one call
/// sends: on macOS `INT_MAX` bytes, beyond which the kernel refuses a send,
/// and elsewhere longer than any slice.
fn datagram_fits(buf: &[u8]) -> io::Result<()> {
    if buf.len() > MAX_LEN {
        return Err(io::Error::from_raw_os_error(libc::EMSGSIZE));
    }
    Ok(())
}

impl Socket {
    /// Connects a new stream socket to `addr`.
    pub(crate) fn connect_stream(addr: &SocketAddr) -> io::Result<Self> {
        let socket = Self::new(family(addr), libc::SOCK_STREAM)?;
        socket.connect(addr)?;
        Ok(socket)
    }

    /// Connects a new stream socket to `addr`, failing after `timeout`, with
    /// a nonblocking `connect` and `poll` as in std. On Linux the socket
    /// starts out nonblocking, saving std's first `ioctl`. A connection that
    /// completes or fails at once ends the call before a zero timeout is
    /// rejected.
    pub(crate) fn connect_stream_timeout(
        addr: &SocketAddr,
        timeout: Duration,
    ) -> io::Result<Self> {
        let socket = Self::new_nonblocking(family(addr), libc::SOCK_STREAM)?;
        let addr = SockAddr::new(addr);
        match socket.connect_once(CAddr::ip(&addr)) {
            Ok(()) => {}
            Err(e) if e.raw_os_error() == Some(libc::EINPROGRESS) => {
                if timeout.is_zero() {
                    return Err(ZERO_TIMEOUT);
                }
                socket.wait_connected(timeout)?;
            }
            Err(e) => return Err(e),
        }
        socket.set_nonblocking(false)?;
        Ok(socket)
    }

    /// Waits up to `timeout` for the connection in progress to complete.
    fn wait_connected(&self, timeout: Duration) -> io::Result<()> {
        let start = monotonic();
        let mut left = timeout;
        let mut pollfd = libc::pollfd {
            fd: self.raw(),
            events: libc::POLLOUT,
            revents: 0,
        };
        // Ends once the attempt completes, fails or runs out of time, or
        // `poll` fails other than by an interruption. Each pass waits for
        // what is left of the timeout, so interruptions keep the deadline.
        loop {
            let (ms, capped) = poll_ms(left);
            // SAFETY: `pollfd` is valid for reads and writes of one entry.
            match unsafe { libc::poll(ptr::from_mut(&mut pollfd), 1, ms) } {
                -1 => {
                    let e = io::Error::last_os_error();
                    if e.raw_os_error() != Some(libc::EINTR) {
                        return Err(e);
                    }
                }
                // `poll` waited for all of `left`.
                0 if !capped => return Err(CONNECT_TIMED_OUT),
                0 => {}
                _ => return self.connect_result(pollfd.revents),
            }
            match timeout.checked_sub(monotonic().saturating_sub(start)) {
                Some(rest) if !rest.is_zero() => left = rest,
                _ => return Err(CONNECT_TIMED_OUT),
            }
        }
    }

    /// The outcome of a connection attempt that `poll` reported finished.
    /// Linux reports a refused connection as `POLLOUT | POLLERR | POLLHUP`,
    /// so the error bits decide, and `SO_ERROR` holds the error.
    fn connect_result(&self, revents: c_short) -> io::Result<()> {
        if revents & (libc::POLLHUP | libc::POLLERR) == 0 {
            return Ok(());
        }
        Err(self.take_error()?.unwrap_or(NO_ERROR_AFTER_POLLHUP))
    }

    /// Binds a new stream socket to `addr` and listens on it.
    pub(crate) fn listen_stream(addr: &SocketAddr) -> io::Result<Self> {
        let socket = Self::new(family(addr), libc::SOCK_STREAM)?;
        // As in std: an address can be bound again right after its previous
        // listener closed, without waiting for its connections to expire.
        socket.set_flag(libc::SOL_SOCKET, libc::SO_REUSEADDR, true)?;
        socket.bind(CAddr::ip(&SockAddr::new(addr)))?;
        socket.listen(BACKLOG)?;
        Ok(socket)
    }

    /// Binds a new datagram socket to `addr`.
    pub(crate) fn bind_datagram(addr: &SocketAddr) -> io::Result<Self> {
        let socket = Self::new(family(addr), libc::SOCK_DGRAM)?;
        socket.bind(CAddr::ip(&SockAddr::new(addr)))?;
        Ok(socket)
    }

    /// Connects to `addr`. After `EINTR` the connection goes on and `connect`
    /// is called again to wait for it, until another result; `EISCONN` then
    /// means that it completed in between.
    pub(crate) fn connect(&self, addr: &SocketAddr) -> io::Result<()> {
        let addr = SockAddr::new(addr);
        loop {
            let Err(e) = self.connect_once(CAddr::ip(&addr)) else {
                return Ok(());
            };
            match e.raw_os_error() {
                Some(libc::EINTR) => {}
                Some(libc::EISCONN) => return Ok(()),
                _ => return Err(e),
            }
        }
    }

    /// Accepts a connection, returning its socket and the peer's address.
    ///
    /// A peer of another family fails with `InvalidInput` after its socket
    /// is closed, as in std.
    pub(crate) fn accept(&self) -> io::Result<(Self, SocketAddr)> {
        let mut storage = Storage::new();
        let (socket, len) = self.accept_into(&mut storage)?;
        let addr = storage.to_socket_addr(len)?;
        Ok((socket, addr))
    }

    /// Returns the address `name` reports, the socket's own or its peer's.
    fn ip_name(&self, name: NameFn) -> io::Result<SocketAddr> {
        let mut storage = Storage::new();
        let len = self.name_into(name, &mut storage)?;
        storage.to_socket_addr(len)
    }

    pub(crate) fn socket_addr(&self) -> io::Result<SocketAddr> {
        self.ip_name(libc::getsockname)
    }

    pub(crate) fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.ip_name(libc::getpeername)
    }

    fn recv_from_flags(
        &self,
        buf: &mut [u8],
        flags: c_int,
    ) -> io::Result<(usize, SocketAddr)> {
        let mut storage = Storage::new();
        let (n, len) = self.recv_from_into(buf, flags, &mut storage)?;
        Ok((n, storage.to_socket_addr(len)?))
    }

    /// Receives a datagram, returning its length and sender.
    pub(crate) fn recv_datagram_from(
        &self,
        buf: &mut [u8],
    ) -> io::Result<(usize, SocketAddr)> {
        self.recv_from_flags(buf, 0)
    }

    pub(crate) fn peek_from(
        &self,
        buf: &mut [u8],
    ) -> io::Result<(usize, SocketAddr)> {
        self.recv_from_flags(buf, libc::MSG_PEEK)
    }

    /// Sends `buf` as one datagram to the connected peer.
    pub(crate) fn send_datagram(&self, buf: &[u8]) -> io::Result<usize> {
        datagram_fits(buf)?;
        self.write(buf)
    }

    /// Sends `buf` as one datagram to `dst`.
    pub(crate) fn send_to(
        &self,
        buf: &[u8],
        dst: &SocketAddr,
    ) -> io::Result<usize> {
        datagram_fits(buf)?;
        self.send_to_addr(buf, CAddr::ip(&SockAddr::new(dst)))
    }

    pub(crate) fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        self.set_flag(libc::IPPROTO_TCP, libc::TCP_NODELAY, nodelay)
    }

    pub(crate) fn nodelay(&self) -> io::Result<bool> {
        self.flag(libc::IPPROTO_TCP, libc::TCP_NODELAY)
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub(crate) fn set_quickack(&self, quickack: bool) -> io::Result<()> {
        self.set_flag(libc::IPPROTO_TCP, libc::TCP_QUICKACK, quickack)
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub(crate) fn quickack(&self) -> io::Result<bool> {
        self.flag(libc::IPPROTO_TCP, libc::TCP_QUICKACK)
    }

    /// Sets an option whose value std passes as an `int` cast from a `u32`,
    /// wrapping: Linux takes -1, `u32::MAX` wrapped, as the default value.
    #[allow(clippy::cast_possible_wrap, reason = "wraps like std")]
    fn set_u32(&self, level: c_int, name: c_int, value: u32) -> io::Result<()> {
        self.setsockopt(level, name, value as c_int)
    }

    /// Reads an option that std reads as an `int` cast to a `u32`.
    #[allow(clippy::cast_sign_loss, reason = "converts like std")]
    fn u32_option(&self, level: c_int, name: c_int) -> io::Result<u32> {
        Ok(self.getsockopt::<c_int>(level, name)? as u32)
    }

    pub(crate) fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.set_u32(libc::IPPROTO_IP, libc::IP_TTL, ttl)
    }

    pub(crate) fn ttl(&self) -> io::Result<u32> {
        self.u32_option(libc::IPPROTO_IP, libc::IP_TTL)
    }

    pub(crate) fn set_broadcast(&self, broadcast: bool) -> io::Result<()> {
        self.set_flag(libc::SOL_SOCKET, libc::SO_BROADCAST, broadcast)
    }

    pub(crate) fn broadcast(&self) -> io::Result<bool> {
        self.flag(libc::SOL_SOCKET, libc::SO_BROADCAST)
    }

    pub(crate) fn set_multicast_loop_v4(&self, on: bool) -> io::Result<()> {
        self.set_flag(libc::IPPROTO_IP, libc::IP_MULTICAST_LOOP, on)
    }

    pub(crate) fn multicast_loop_v4(&self) -> io::Result<bool> {
        self.flag(libc::IPPROTO_IP, libc::IP_MULTICAST_LOOP)
    }

    pub(crate) fn set_multicast_ttl_v4(&self, ttl: u32) -> io::Result<()> {
        self.set_u32(libc::IPPROTO_IP, libc::IP_MULTICAST_TTL, ttl)
    }

    pub(crate) fn multicast_ttl_v4(&self) -> io::Result<u32> {
        self.u32_option(libc::IPPROTO_IP, libc::IP_MULTICAST_TTL)
    }

    pub(crate) fn set_multicast_loop_v6(&self, on: bool) -> io::Result<()> {
        self.set_flag(libc::IPPROTO_IPV6, libc::IPV6_MULTICAST_LOOP, on)
    }

    pub(crate) fn multicast_loop_v6(&self) -> io::Result<bool> {
        self.flag(libc::IPPROTO_IPV6, libc::IPV6_MULTICAST_LOOP)
    }

    /// Joins or leaves an IPv4 multicast group, as `name` says.
    fn membership_v4(
        &self,
        name: c_int,
        multiaddr: Ipv4Addr,
        interface: Ipv4Addr,
    ) -> io::Result<()> {
        // `s_addr` holds the address in network order, as its octets are.
        let in_addr = |ip: Ipv4Addr| libc::in_addr {
            s_addr: u32::from_ne_bytes(ip.octets()),
        };
        let mreq = libc::ip_mreq {
            imr_multiaddr: in_addr(multiaddr),
            imr_interface: in_addr(interface),
        };
        self.setsockopt(libc::IPPROTO_IP, name, mreq)
    }

    /// Joins or leaves an IPv6 multicast group, as `name` says.
    fn membership_v6(
        &self,
        name: c_int,
        multiaddr: &Ipv6Addr,
        interface: u32,
    ) -> io::Result<()> {
        // Android declares the interface index as an `int`.
        #[cfg(target_os = "android")]
        #[allow(clippy::cast_possible_wrap, reason = "wraps like std")]
        let interface = interface as c_int;
        let mreq = libc::ipv6_mreq {
            ipv6mr_multiaddr: libc::in6_addr {
                s6_addr: multiaddr.octets(),
            },
            ipv6mr_interface: interface,
        };
        self.setsockopt(libc::IPPROTO_IPV6, name, mreq)
    }

    pub(crate) fn join_multicast_v4(
        &self,
        multiaddr: Ipv4Addr,
        interface: Ipv4Addr,
    ) -> io::Result<()> {
        self.membership_v4(libc::IP_ADD_MEMBERSHIP, multiaddr, interface)
    }

    pub(crate) fn leave_multicast_v4(
        &self,
        multiaddr: Ipv4Addr,
        interface: Ipv4Addr,
    ) -> io::Result<()> {
        self.membership_v4(libc::IP_DROP_MEMBERSHIP, multiaddr, interface)
    }

    pub(crate) fn join_multicast_v6(
        &self,
        multiaddr: &Ipv6Addr,
        interface: u32,
    ) -> io::Result<()> {
        self.membership_v6(IPV6_ADD_MEMBERSHIP, multiaddr, interface)
    }

    pub(crate) fn leave_multicast_v6(
        &self,
        multiaddr: &Ipv6Addr,
        interface: u32,
    ) -> io::Result<()> {
        self.membership_v6(IPV6_DROP_MEMBERSHIP, multiaddr, interface)
    }
}
