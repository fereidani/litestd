//! TCP and UDP sockets.

use core::{ptr, time::Duration};

use windows_sys::Win32::Networking::WinSock::{
    self as ws, FD_SET, FD_SETSIZE, IN_ADDR, IN_ADDR_0, IN6_ADDR, IN6_ADDR_0,
    INVALID_SOCKET, IP_ADD_MEMBERSHIP, IP_DROP_MEMBERSHIP, IP_MREQ,
    IP_MULTICAST_LOOP, IP_MULTICAST_TTL, IP_TTL, IPPROTO_IP, IPPROTO_IPV6,
    IPPROTO_TCP, IPV6_ADD_MEMBERSHIP, IPV6_DROP_MEMBERSHIP, IPV6_MREQ,
    IPV6_MULTICAST_LOOP, MSG_PEEK, SO_BROADCAST, SOCK_DGRAM, SOCK_STREAM,
    SOCKADDR, SOCKET, SOL_SOCKET, TCP_NODELAY, TIMEVAL, WSAEMSGSIZE,
    WSAEWOULDBLOCK,
};

use super::{
    AF_INET, AF_INET6, cvt, cvt_len, init, last_error, owned,
    socket::{Socket, clamp, shutdown_as_eof},
};
use crate::{
    io,
    net::{
        CONNECT_TIMED_OUT, Ipv4Addr, Ipv6Addr, SocketAddr, ZERO_TIMEOUT,
        sockaddr::{SockAddr, Storage},
    },
};

/// The backlog of a listening TCP socket, std's.
const BACKLOG: i32 = 128;

/// The signature that `bind` and `connect` share.
type AddrFn = unsafe extern "system" fn(SOCKET, *const SOCKADDR, i32) -> i32;

/// The signature that `getsockname` and `getpeername` share.
type NameFn = unsafe extern "system" fn(SOCKET, *mut SOCKADDR, *mut i32) -> i32;

/// The family of `addr`, for `WSASocketW`.
const fn family(addr: &SocketAddr) -> u16 {
    match addr {
        SocketAddr::V4(_) => AF_INET,
        SocketAddr::V6(_) => AF_INET6,
    }
}

/// The length of an address Winsock stored; a negative one means none.
fn addr_len(len: i32) -> usize {
    usize::try_from(len).unwrap_or(0)
}

/// Returns the length of a datagram to send, which Winsock takes as an
/// `i32`: a longer one fails with `WSAEMSGSIZE`, as in std.
fn datagram_len(buf: &[u8]) -> io::Result<i32> {
    i32::try_from(buf.len())
        .map_err(|_| io::Error::from_raw_os_error(WSAEMSGSIZE))
}

/// Converts an IPv4 address to Winsock's network-order `IN_ADDR`.
const fn in_addr(ip: Ipv4Addr) -> IN_ADDR {
    IN_ADDR {
        S_un: IN_ADDR_0 {
            S_addr: u32::from_ne_bytes(ip.octets()),
        },
    }
}

/// Defines the getter and setter of a socket option that std types as `bool`
/// (`flag`) or as `u32`, which std passes as an `int` with the same bits.
/// Winsock takes a flag as a `BOOL` or a `DWORD`, and any byte set makes it
/// true.
macro_rules! option {
    (flag $get:ident, $set:ident, $level:ident, $name:ident) => {
        pub(crate) fn $set(&self, on: bool) -> io::Result<()> {
            self.set_option($level, $name, u32::from(on))
        }

        pub(crate) fn $get(&self) -> io::Result<bool> {
            Ok(self.option($level, $name)? != [0; 4])
        }
    };
    (u32 $get:ident, $set:ident, $level:ident, $name:ident) => {
        pub(crate) fn $set(&self, value: u32) -> io::Result<()> {
            self.set_option($level, $name, value)
        }

        pub(crate) fn $get(&self) -> io::Result<u32> {
            Ok(u32::from_ne_bytes(self.option($level, $name)?))
        }
    };
}

impl Socket {
    /// Starts Winsock and creates a socket of type `ty` for `addr`'s family.
    fn for_addr(addr: &SocketAddr, ty: i32) -> io::Result<Self> {
        init()?;
        Self::new(family(addr), ty)
    }

    /// Connects a new stream socket to `addr`.
    pub(crate) fn connect_stream(addr: &SocketAddr) -> io::Result<Self> {
        let socket = Self::for_addr(addr, SOCK_STREAM)?;
        socket.connect(addr)?;
        Ok(socket)
    }

    /// Connects a new stream socket to `addr`, failing after `timeout`, as
    /// std does: a nonblocking connect, then `select`. A connection that
    /// completes or fails at once returns before a zero timeout is rejected.
    pub(crate) fn connect_stream_timeout(
        addr: &SocketAddr,
        timeout: Duration,
    ) -> io::Result<Self> {
        let socket = Self::for_addr(addr, SOCK_STREAM)?;
        socket.set_nonblocking(true)?;
        let result = socket.connect(addr);
        socket.set_nonblocking(false)?;
        match result {
            Err(e) if e.raw_os_error() == Some(WSAEWOULDBLOCK) => {
                if timeout.is_zero() {
                    return Err(ZERO_TIMEOUT);
                }
                socket.wait_connected(timeout)?;
            }
            result => result?,
        }
        Ok(socket)
    }

    /// Waits up to `timeout` for the connection in progress to complete.
    fn wait_connected(&self, timeout: Duration) -> io::Result<()> {
        // The microseconds are below one million.
        #[allow(clippy::cast_possible_wrap)]
        let mut tv = TIMEVAL {
            tv_sec: i32::try_from(timeout.as_secs()).unwrap_or(i32::MAX),
            tv_usec: timeout.subsec_micros() as i32,
        };
        // `select` would not wait at all for less than a microsecond.
        if tv.tv_sec == 0 && tv.tv_usec == 0 {
            tv.tv_usec = 1;
        }
        let mut fd_array = [0; FD_SETSIZE as usize];
        fd_array[0] = self.raw();
        let mut writable = FD_SET {
            fd_count: 1,
            fd_array,
        };
        let mut failed = FD_SET {
            fd_count: 1,
            fd_array,
        };
        // SAFETY: both sets are valid for reads and writes and hold only
        // this open socket, and `tv` is valid for reads.
        let count = cvt_len(unsafe {
            ws::select(
                1,
                ptr::null_mut(),
                &raw mut writable,
                &raw mut failed,
                &raw const tv,
            )
        })?;
        if count == 0 {
            return Err(CONNECT_TIMED_OUT);
        }
        // SAFETY: `writable` is a valid set, which `FD_ISSET` only reads.
        let connected =
            unsafe { ws::__WSAFDIsSet(self.raw(), &raw mut writable) } != 0;
        // A socket that is not writable failed to connect, with the error
        // in `SO_ERROR`; as in std, none there counts as connected.
        if connected {
            Ok(())
        } else {
            self.take_error()?.map_or(Ok(()), Err)
        }
    }

    /// Binds a new stream socket to `addr` and listens on it. Like std, this
    /// skips `SO_REUSEADDR`, which on Windows allows taking an address in use.
    pub(crate) fn listen_stream(addr: &SocketAddr) -> io::Result<Self> {
        let socket = Self::for_addr(addr, SOCK_STREAM)?;
        socket.with_addr(addr, ws::bind)?;
        // SAFETY: the socket is open; `listen` takes no pointers.
        cvt(unsafe { ws::listen(socket.raw(), BACKLOG) })?;
        Ok(socket)
    }

    /// Binds a new datagram socket to `addr`.
    pub(crate) fn bind_datagram(addr: &SocketAddr) -> io::Result<Self> {
        let socket = Self::for_addr(addr, SOCK_DGRAM)?;
        socket.with_addr(addr, ws::bind)?;
        Ok(socket)
    }

    /// Calls `bind` or `connect` with `addr`.
    fn with_addr(&self, addr: &SocketAddr, f: AddrFn) -> io::Result<()> {
        let addr = SockAddr::new(addr);
        // SAFETY: the socket is open, and `addr` points to `addr.len()`
        // initialized bytes, valid during the call.
        cvt(unsafe { f(self.raw(), addr.as_ptr(), addr.len().into()) })
    }

    /// Connects to `addr`, for a stream or a datagram socket.
    pub(crate) fn connect(&self, addr: &SocketAddr) -> io::Result<()> {
        self.with_addr(addr, ws::connect)
    }

    /// Accepts a connection, returning its socket and the peer's address; a
    /// peer of another family fails with `InvalidInput`, as in std.
    pub(crate) fn accept(&self) -> io::Result<(Self, SocketAddr)> {
        let mut storage = Storage::new();
        let mut len = Storage::LEN.into();
        // SAFETY: the socket is open, `storage` is valid for writes of
        // `len` bytes, and `len` for reads and writes of an `i32`.
        let raw = unsafe {
            ws::accept(self.raw(), storage.as_mut_ptr(), &raw mut len)
        };
        if raw == INVALID_SOCKET {
            return Err(last_error());
        }
        // SAFETY: `accept` returned a new socket that nothing else owns.
        let socket = Self::from_inner(unsafe { owned(raw) });
        let addr = storage.to_socket_addr(addr_len(len))?;
        Ok((socket, addr))
    }

    /// Returns the address `name` reports, the socket's own or its peer's.
    fn ip_name(&self, name: NameFn) -> io::Result<SocketAddr> {
        let mut storage = Storage::new();
        let mut len = Storage::LEN.into();
        // SAFETY: the socket is open, `storage` is valid for writes of
        // `len` bytes, and `len` for reads and writes of an `i32`.
        cvt(unsafe { name(self.raw(), storage.as_mut_ptr(), &raw mut len) })?;
        storage.to_socket_addr(addr_len(len))
    }

    pub(crate) fn socket_addr(&self) -> io::Result<SocketAddr> {
        self.ip_name(ws::getsockname)
    }

    pub(crate) fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.ip_name(ws::getpeername)
    }

    /// Receives a datagram with `flags`, returning its length and sender. As
    /// in std, a socket shut down for reading reports the sender Winsock
    /// stored, failing with `InvalidInput` if there is none.
    fn recv_from_flags(
        &self,
        buf: &mut [u8],
        flags: i32,
    ) -> io::Result<(usize, SocketAddr)> {
        let mut storage = Storage::new();
        let mut len = Storage::LEN.into();
        // SAFETY: the socket is open, `buf` is writable for the length
        // passed, `storage` for `len` bytes, and `len` for reads and writes.
        let result = unsafe {
            ws::recvfrom(
                self.raw(),
                buf.as_mut_ptr(),
                clamp(buf.len()),
                flags,
                storage.as_mut_ptr(),
                &raw mut len,
            )
        };
        let n = match usize::try_from(result) {
            Ok(n) => n.min(buf.len()),
            Err(_) => shutdown_as_eof()?,
        };
        Ok((n, storage.to_socket_addr(addr_len(len))?))
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
        self.recv_from_flags(buf, MSG_PEEK)
    }

    /// Sends `buf` as one datagram to the connected peer.
    pub(crate) fn send_datagram(&self, buf: &[u8]) -> io::Result<usize> {
        datagram_len(buf)?;
        self.write(buf)
    }

    /// Sends `buf` as one datagram to `dst`.
    pub(crate) fn send_to(
        &self,
        buf: &[u8],
        dst: &SocketAddr,
    ) -> io::Result<usize> {
        let len = datagram_len(buf)?;
        let dst = SockAddr::new(dst);
        let (dst, dst_len) = (dst.as_ptr(), dst.len().into());
        // SAFETY: the socket is open, `buf` is valid for reads of `len`
        // bytes, and `dst` points to `dst_len` initialized bytes.
        cvt_len(unsafe {
            ws::sendto(self.raw(), buf.as_ptr(), len, 0, dst, dst_len)
        })
    }

    option!(flag nodelay, set_nodelay, IPPROTO_TCP, TCP_NODELAY);
    option!(flag broadcast, set_broadcast, SOL_SOCKET, SO_BROADCAST);
    option!(flag multicast_loop_v4, set_multicast_loop_v4, IPPROTO_IP,
            IP_MULTICAST_LOOP);
    option!(flag multicast_loop_v6, set_multicast_loop_v6, IPPROTO_IPV6,
            IPV6_MULTICAST_LOOP);
    option!(u32 ttl, set_ttl, IPPROTO_IP, IP_TTL);
    option!(u32 multicast_ttl_v4, set_multicast_ttl_v4, IPPROTO_IP,
            IP_MULTICAST_TTL);

    /// Joins or leaves an IPv4 multicast group, as `name` says.
    fn membership_v4(
        &self,
        name: i32,
        multiaddr: Ipv4Addr,
        interface: Ipv4Addr,
    ) -> io::Result<()> {
        let mreq = IP_MREQ {
            imr_multiaddr: in_addr(multiaddr),
            imr_interface: in_addr(interface),
        };
        self.set_option(IPPROTO_IP, name, mreq)
    }

    /// Joins or leaves an IPv6 multicast group, as `name` says.
    fn membership_v6(
        &self,
        name: i32,
        multiaddr: &Ipv6Addr,
        interface: u32,
    ) -> io::Result<()> {
        let mreq = IPV6_MREQ {
            ipv6mr_multiaddr: IN6_ADDR {
                u: IN6_ADDR_0 {
                    Byte: multiaddr.octets(),
                },
            },
            ipv6mr_interface: interface,
        };
        self.set_option(IPPROTO_IPV6, name, mreq)
    }

    pub(crate) fn join_multicast_v4(
        &self,
        multiaddr: Ipv4Addr,
        interface: Ipv4Addr,
    ) -> io::Result<()> {
        self.membership_v4(IP_ADD_MEMBERSHIP, multiaddr, interface)
    }

    pub(crate) fn leave_multicast_v4(
        &self,
        multiaddr: Ipv4Addr,
        interface: Ipv4Addr,
    ) -> io::Result<()> {
        self.membership_v4(IP_DROP_MEMBERSHIP, multiaddr, interface)
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
