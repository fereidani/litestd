//! Android-specific networking functionality.

#[cfg(feature = "path")]
pub use crate::net::linux_ext::SocketAddrExt;
pub use crate::net::linux_ext::TcpStreamExt;
