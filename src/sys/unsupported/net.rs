//! Sockets where the platform has none: creating one fails, so no socket
//! exists to call the other methods on.

use core::{convert::Infallible, time::Duration};

use alloc_crate::{string::String, vec::Vec};

pub(crate) use super::UNSUPPORTED;
use crate::{
    io::{self, IoSlice, IoSliceMut},
    net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr},
};

/// A socket, of which there are none.
pub(crate) struct Socket(Infallible);

/// The addresses a host name resolves to, never produced.
pub(crate) struct LookupHost(Infallible);

impl Iterator for LookupHost {
    type Item = SocketAddr;

    fn next(&mut self) -> Option<SocketAddr> {
        match self.0 {}
    }
}

/// Fails: there is no resolver.
pub(crate) fn lookup_host(_host: &str, _port: u16) -> io::Result<LookupHost> {
    Err(UNSUPPORTED)
}

impl Socket {
    pub(crate) fn connect_stream(_addr: &SocketAddr) -> io::Result<Self> {
        Err(UNSUPPORTED)
    }

    pub(crate) fn connect_stream_timeout(
        _addr: &SocketAddr,
        _timeout: Duration,
    ) -> io::Result<Self> {
        Err(UNSUPPORTED)
    }

    pub(crate) fn listen_stream(_addr: &SocketAddr) -> io::Result<Self> {
        Err(UNSUPPORTED)
    }

    pub(crate) fn bind_datagram(_addr: &SocketAddr) -> io::Result<Self> {
        Err(UNSUPPORTED)
    }

    pub(crate) fn raw_debug(&self) -> (&'static str, i32) {
        match self.0 {}
    }
}

unreachable_methods! {
    Socket;
    fn duplicate(&self) -> io::Result<Self>;
    fn read(&self, buf: &mut [u8]) -> io::Result<usize>;
    fn peek(&self, buf: &mut [u8]) -> io::Result<usize>;
    fn read_to_end(&self, buf: &mut Vec<u8>) -> io::Result<usize>;
    fn read_to_string(&self, buf: &mut String) -> io::Result<usize>;
    fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize>;
    fn write(&self, buf: &[u8]) -> io::Result<usize>;
    fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize>;
    fn shutdown(&self, how: Shutdown) -> io::Result<()>;
    fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()>;
    fn set_read_timeout(&self, dur: Option<Duration>) -> io::Result<()>;
    fn set_write_timeout(&self, dur: Option<Duration>) -> io::Result<()>;
    fn read_timeout(&self) -> io::Result<Option<Duration>>;
    fn write_timeout(&self) -> io::Result<Option<Duration>>;
    fn take_error(&self) -> io::Result<Option<io::Error>>;
    fn connect(&self, addr: &SocketAddr) -> io::Result<()>;
    fn accept(&self) -> io::Result<(Self, SocketAddr)>;
    fn socket_addr(&self) -> io::Result<SocketAddr>;
    fn peer_addr(&self) -> io::Result<SocketAddr>;
    fn recv_datagram_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)>;
    fn peek_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)>;
    fn send_datagram(&self, buf: &[u8]) -> io::Result<usize>;
    fn send_to(&self, buf: &[u8], dst: &SocketAddr) -> io::Result<usize>;
    fn set_nodelay(&self, nodelay: bool) -> io::Result<()>;
    fn nodelay(&self) -> io::Result<bool>;
    fn set_ttl(&self, ttl: u32) -> io::Result<()>;
    fn ttl(&self) -> io::Result<u32>;
    fn set_broadcast(&self, broadcast: bool) -> io::Result<()>;
    fn broadcast(&self) -> io::Result<bool>;
    fn set_multicast_loop_v4(&self, on: bool) -> io::Result<()>;
    fn multicast_loop_v4(&self) -> io::Result<bool>;
    fn set_multicast_ttl_v4(&self, ttl: u32) -> io::Result<()>;
    fn multicast_ttl_v4(&self) -> io::Result<u32>;
    fn set_multicast_loop_v6(&self, on: bool) -> io::Result<()>;
    fn multicast_loop_v6(&self) -> io::Result<bool>;
    fn join_multicast_v4(&self, multiaddr: Ipv4Addr, interface: Ipv4Addr) -> io::Result<()>;
    fn leave_multicast_v4(&self, multiaddr: Ipv4Addr, interface: Ipv4Addr) -> io::Result<()>;
    fn join_multicast_v6(&self, multiaddr: &Ipv6Addr, interface: u32) -> io::Result<()>;
    fn leave_multicast_v6(&self, multiaddr: &Ipv6Addr, interface: u32) -> io::Result<()>;
}
