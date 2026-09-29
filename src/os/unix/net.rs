//! Unix-specific networking functionality.
//!
//! The sockets are opened close-on-exec. Paths name them, so the module
//! needs the `path` feature as well as `net`.

mod addr;
mod datagram;
mod listener;
mod stream;

pub use self::{
    addr::SocketAddr,
    datagram::UnixDatagram,
    listener::{Incoming, UnixListener},
    stream::UnixStream,
};
use crate::{
    fmt,
    sys::{net::Socket, pipe::FileDesc},
};

/// Formats a socket as std does: `Name { fd: .., local: .., peer: .. }`,
/// leaving out the addresses the OS does not report.
fn debug_socket(
    f: &mut fmt::Formatter<'_>,
    name: &str,
    socket: &Socket,
    peer: bool,
) -> fmt::Result {
    let mut builder = f.debug_struct(name);
    builder.field("fd", &FileDesc(socket.as_inner()));
    if let Ok(inner) = socket.unix_socket_addr() {
        builder.field("local", &SocketAddr { inner });
    }
    if peer {
        if let Ok(inner) = socket.unix_peer_addr() {
            builder.field("peer", &SocketAddr { inner });
        }
    }
    builder.finish()
}
