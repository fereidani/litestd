//! The Linux and Android networking extension traits, which
//! `os::linux::net` and `os::android::net` export.

use crate::{io, net::TcpStream};
#[cfg(feature = "path")]
use crate::{
    os::unix::net::SocketAddr, sys::net::AddrKind, sys::net::UnixAddr,
};

mod private {
    /// Seals the extension traits, as in std, so litestd may add methods.
    pub trait Sealed {}

    impl Sealed for crate::net::TcpStream {}
    #[cfg(feature = "path")]
    impl Sealed for crate::os::unix::net::SocketAddr {}
}

/// Os-specific extensions for [`TcpStream`].
///
/// This trait is sealed: it cannot be implemented outside litestd.
pub trait TcpStreamExt: private::Sealed {
    /// Enables or disables `TCP_QUICKACK`, which makes Linux send ACKs
    /// eagerly instead of delaying them. Linux may reset it later.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    fn set_quickack(&self, quickack: bool) -> io::Result<()>;

    /// Gets the value of the `TCP_QUICKACK` option on this socket.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    fn quickack(&self) -> io::Result<bool>;
}

impl TcpStreamExt for TcpStream {
    fn set_quickack(&self, quickack: bool) -> io::Result<()> {
        self.inner.set_quickack(quickack)
    }

    fn quickack(&self) -> io::Result<bool> {
        self.inner.quickack()
    }
}

/// Platform-specific extensions to [`SocketAddr`].
///
/// This trait is sealed: it cannot be implemented outside litestd.
#[cfg(feature = "path")]
pub trait SocketAddrExt: private::Sealed {
    /// Creates a Unix socket address in the abstract namespace, which needs
    /// no filesystem entry. The name may contain any bytes, including zero.
    ///
    /// # Errors
    ///
    /// Returns an error if the name is longer than `SUN_LEN - 1`.
    fn from_abstract_name<N>(name: N) -> io::Result<SocketAddr>
    where
        N: AsRef<[u8]>;

    /// Returns the contents of this address if it is in the abstract
    /// namespace.
    fn as_abstract_name(&self) -> Option<&[u8]>;
}

#[cfg(feature = "path")]
impl SocketAddrExt for SocketAddr {
    fn from_abstract_name<N>(name: N) -> io::Result<Self>
    where
        N: AsRef<[u8]>,
    {
        UnixAddr::from_abstract_name(name.as_ref()).map(|inner| Self { inner })
    }

    fn as_abstract_name(&self) -> Option<&[u8]> {
        match self.inner.kind() {
            AddrKind::Abstract(name) => Some(name),
            _ => None,
        }
    }
}
