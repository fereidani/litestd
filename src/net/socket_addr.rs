//! [`ToSocketAddrs`], and the address iteration of `connect` and `bind`.

use core::{iter, option, slice};

use alloc_crate::{string::String, vec, vec::Vec};

use crate::{
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    sys,
};

/// A trait for objects which can be converted or resolved to one or more
/// [`SocketAddr`] values.
///
/// It is implemented for the socket address types, `(ip, port)` and
/// `(host, port)` tuples, `"host:port"` strings and slices of
/// [`SocketAddr`]. A string that parses as an IP or socket address needs no
/// name resolution; anything else goes to the system resolver, and the
/// addresses it returns that are not IP addresses are ignored.
pub trait ToSocketAddrs {
    /// Returned iterator over socket addresses which this type may
    /// correspond to.
    type Iter: Iterator<Item = SocketAddr>;

    /// Converts this object to an iterator of resolved [`SocketAddr`]s,
    /// which may be empty. Blocks while name resolution runs.
    ///
    /// # Errors
    ///
    /// Fails if the value is not a valid address, or if name resolution
    /// fails.
    fn to_socket_addrs(&self) -> io::Result<Self::Iter>;
}

impl ToSocketAddrs for SocketAddr {
    type Iter = option::IntoIter<Self>;

    fn to_socket_addrs(&self) -> io::Result<option::IntoIter<Self>> {
        Ok(Some(*self).into_iter())
    }
}

/// Implements `ToSocketAddrs` for `Copy` types that convert to one address.
macro_rules! impl_single_addr {
    ($($t:ty),*) => {$(
        impl ToSocketAddrs for $t {
            type Iter = option::IntoIter<SocketAddr>;

            fn to_socket_addrs(&self) -> io::Result<Self::Iter> {
                Ok(Some(SocketAddr::from(*self)).into_iter())
            }
        }
    )*};
}

impl_single_addr!(
    SocketAddrV4,
    SocketAddrV6,
    (IpAddr, u16),
    (Ipv4Addr, u16),
    (Ipv6Addr, u16)
);

impl ToSocketAddrs for (&str, u16) {
    type Iter = vec::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> io::Result<vec::IntoIter<SocketAddr>> {
        resolve_host(self.0, self.1)
    }
}

impl ToSocketAddrs for (String, u16) {
    type Iter = vec::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> io::Result<vec::IntoIter<SocketAddr>> {
        resolve_host(&self.0, self.1)
    }
}

/// Accepts strings like `"localhost:12345"`.
impl ToSocketAddrs for str {
    type Iter = vec::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> io::Result<vec::IntoIter<SocketAddr>> {
        resolve_str(self)
    }
}

impl<'a> ToSocketAddrs for &'a [SocketAddr] {
    type Iter = iter::Cloned<slice::Iter<'a, SocketAddr>>;

    #[allow(clippy::cloned_instead_of_copied, reason = "std's iterator type")]
    fn to_socket_addrs(&self) -> io::Result<Self::Iter> {
        Ok(self.iter().cloned())
    }
}

impl<T: ToSocketAddrs + ?Sized> ToSocketAddrs for &T {
    type Iter = T::Iter;

    fn to_socket_addrs(&self) -> io::Result<T::Iter> {
        (**self).to_socket_addrs()
    }
}

impl ToSocketAddrs for String {
    type Iter = vec::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> io::Result<vec::IntoIter<SocketAddr>> {
        resolve_str(self)
    }
}

/// Resolves `host` with `port`: an IP address as it is, a host name with
/// the system resolver.
fn resolve_host(
    host: &str,
    port: u16,
) -> io::Result<vec::IntoIter<SocketAddr>> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(ip, port)].into_iter());
    }
    lookup(host, port)
}

/// Resolves a socket address as it is, and anything else as `<host>:<port>`
/// with the system resolver, as std does even for strings like `::1:80`.
fn resolve_str(addr: &str) -> io::Result<vec::IntoIter<SocketAddr>> {
    if let Ok(addr) = addr.parse::<SocketAddr>() {
        return Ok(vec![addr].into_iter());
    }
    let Some((host, port)) = addr.rsplit_once(':') else {
        return Err(io::const_error!(
            io::ErrorKind::InvalidInput,
            "invalid socket address",
        ));
    };
    let Ok(port) = port.parse::<u16>() else {
        return Err(io::const_error!(
            io::ErrorKind::InvalidInput,
            "invalid port value",
        ));
    };
    lookup(host, port)
}

#[allow(clippy::needless_collect, reason = "the iterator type is std's")]
fn lookup(host: &str, port: u16) -> io::Result<vec::IntoIter<SocketAddr>> {
    let addrs: Vec<SocketAddr> = sys::net::lookup_host(host, port)?.collect();
    Ok(addrs.into_iter())
}

/// The error of `connect` and `bind` for an address that resolves to nothing.
#[cfg(any(unix, windows))]
const NO_ADDRESSES: io::Error = io::const_error!(
    io::ErrorKind::InvalidInput,
    "could not resolve to any addresses",
);

/// Calls `f` with each address `addr` resolves to until a call succeeds,
/// returning the first success or the error of the last attempt.
///
/// The loop takes the addresses as a trait object, so all address types
/// share one copy of it per operation.
pub(crate) fn each_addr<A: ToSocketAddrs, T>(
    addr: A,
    f: impl FnMut(&SocketAddr) -> io::Result<T>,
) -> io::Result<T> {
    // Where the platform has no sockets, std fails before it resolves.
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (addr, f);
        Err(sys::net::UNSUPPORTED)
    }
    #[cfg(any(unix, windows))]
    try_each(&mut addr.to_socket_addrs()?, f)
}

#[cfg(any(unix, windows))]
fn try_each<T>(
    addrs: &mut dyn Iterator<Item = SocketAddr>,
    mut f: impl FnMut(&SocketAddr) -> io::Result<T>,
) -> io::Result<T> {
    let mut last_err = None;
    for addr in addrs {
        match f(&addr) {
            Ok(value) => return Ok(value),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or(NO_ADDRESSES))
}
