//! Unix domain sockets, for `os::unix::net`.

use core::{ffi::c_int, marker::PhantomData, mem::offset_of, ptr};

#[cfg(target_vendor = "apple")]
use super::os;
use super::{AddrRoom, CAddr, Socket, cvt};
use crate::{
    io,
    os::fd::{FromRawFd, OwnedFd},
};

/// Where the path starts in a `sockaddr_un`, and the length of an unnamed
/// address.
const SUN_PATH_OFFSET: usize = offset_of!(libc::sockaddr_un, sun_path);

/// The room for the path in a `sockaddr_un`: 108 bytes on Linux, 104 on
/// macOS and the BSDs.
const SUN_PATH_LEN: usize = size_of::<libc::sockaddr_un>() - SUN_PATH_OFFSET;

/// `sockaddr_un`, with the path as bytes, which accept any byte pattern.
#[repr(C)]
#[derive(Clone, Copy)]
struct SockaddrUn {
    /// The `sun_len` of macOS and the BSDs, which the kernel ignores in the
    /// addresses it takes and which std leaves zero.
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    len: u8,
    family: libc::sa_family_t,
    path: [u8; SUN_PATH_LEN],
}

const _: () = {
    assert!(size_of::<SockaddrUn>() == size_of::<libc::sockaddr_un>());
    assert!(align_of::<SockaddrUn>() == align_of::<libc::sockaddr_un>());
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    assert!(
        offset_of!(SockaddrUn, len) == offset_of!(libc::sockaddr_un, sun_len)
    );
    assert!(
        offset_of!(SockaddrUn, family)
            == offset_of!(libc::sockaddr_un, sun_family)
    );
    assert!(offset_of!(SockaddrUn, path) == SUN_PATH_OFFSET);
};

impl SockaddrUn {
    /// An `AF_UNIX` address with an empty path.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a small positive constant"
    )]
    const UNIX: Self = Self {
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        len: 0,
        family: libc::AF_UNIX as libc::sa_family_t,
        path: [0; SUN_PATH_LEN],
    };

    /// Zeros, for the kernel to fill in.
    const ZERO: Self = Self {
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        len: 0,
        family: 0,
        path: [0; SUN_PATH_LEN],
    };
}

impl AddrRoom for SockaddrUn {
    fn as_mut_c(&mut self) -> (*mut libc::sockaddr, libc::socklen_t) {
        (ptr::from_mut(self).cast(), socklen(size_of::<Self>()))
    }
}

/// Converts a length of at most `size_of::<SockaddrUn>()`.
#[allow(clippy::cast_possible_truncation, reason = "at most 110")]
const fn socklen(len: usize) -> libc::socklen_t {
    len as libc::socklen_t
}

/// The address of a Unix domain socket: a `sockaddr_un` and its length,
/// from `SUN_PATH_OFFSET`, unnamed, to the size of `sockaddr_un`.
#[derive(Clone, Copy)]
pub(crate) struct UnixAddr {
    addr: SockaddrUn,
    len: libc::socklen_t,
}

/// What a [`UnixAddr`] names.
pub(crate) enum AddrKind<'a> {
    Unnamed,
    Pathname(&'a [u8]),
    /// A name in Linux's abstract namespace, which macOS lacks.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    Abstract(&'a [u8]),
}

/// The type of a Unix domain socket.
#[derive(Clone, Copy)]
pub(crate) enum UnixType {
    Stream,
    Datagram,
}

impl UnixType {
    const fn raw(self) -> c_int {
        match self {
            Self::Stream => libc::SOCK_STREAM,
            Self::Datagram => libc::SOCK_DGRAM,
        }
    }
}

const INTERIOR_NUL: io::Error = io::const_error!(
    io::ErrorKind::InvalidInput,
    "paths must not contain interior null bytes",
);

const PATH_TOO_LONG: io::Error = io::const_error!(
    io::ErrorKind::InvalidInput,
    "path must be shorter than SUN_LEN",
);

#[cfg(any(target_os = "linux", target_os = "android"))]
const NAME_TOO_LONG: io::Error = io::const_error!(
    io::ErrorKind::InvalidInput,
    "abstract socket name must be shorter than SUN_LEN",
);

const NOT_UNIX: io::Error = io::const_error!(
    io::ErrorKind::InvalidInput,
    "file descriptor did not correspond to a Unix socket",
);

impl UnixAddr {
    /// The address of the socket file at `path`, which must be shorter than
    /// the room in a `sockaddr_un`, to leave space for the NUL that std
    /// counts in the length. An empty path makes an unnamed address.
    pub(crate) fn from_pathname(path: &[u8]) -> io::Result<Self> {
        if path.contains(&0) {
            return Err(INTERIOR_NUL);
        }
        if path.len() >= SUN_PATH_LEN {
            return Err(PATH_TOO_LONG);
        }
        let mut addr = SockaddrUn::UNIX;
        addr.path[..path.len()].copy_from_slice(path);
        let nul = usize::from(!path.is_empty());
        let len = socklen(SUN_PATH_OFFSET + path.len() + nul);
        Ok(Self { addr, len })
    }

    /// The address `name` in the abstract namespace, which follows a NUL in
    /// the path.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub(crate) fn from_abstract_name(name: &[u8]) -> io::Result<Self> {
        if name.len() >= SUN_PATH_LEN {
            return Err(NAME_TOO_LONG);
        }
        let mut addr = SockaddrUn::UNIX;
        addr.path[1..=name.len()].copy_from_slice(name);
        let len = socklen(SUN_PATH_OFFSET + 1 + name.len());
        Ok(Self { addr, len })
    }

    /// Checks an address the kernel returned with the length it reported.
    fn from_parts(addr: SockaddrUn, len: usize) -> io::Result<Self> {
        // Linux reports no address at all for a datagram from an unbound
        // socket, and macOS a zeroed path; std treats both as unnamed.
        if len == 0 {
            let len = socklen(SUN_PATH_OFFSET);
            return Ok(Self { addr, len });
        }
        if addr.family != SockaddrUn::UNIX.family {
            return Err(NOT_UNIX);
        }
        // Linux reports at least the family and never more than it stored.
        let len = len.clamp(SUN_PATH_OFFSET, size_of::<SockaddrUn>());
        Ok(Self {
            addr,
            len: socklen(len),
        })
    }

    pub(crate) fn kind(&self) -> AddrKind<'_> {
        let len = (self.len as usize).saturating_sub(SUN_PATH_OFFSET);
        let path = self.addr.path.get(..len).unwrap_or(&self.addr.path);
        match path.split_first() {
            None => AddrKind::Unnamed,
            #[cfg(any(target_os = "linux", target_os = "android"))]
            Some((0, name)) => AddrKind::Abstract(name),
            // macOS and the BSDs report an unnamed socket with a zeroed
            // path.
            #[cfg(not(any(target_os = "linux", target_os = "android")))]
            Some((0, _)) => AddrKind::Unnamed,
            // Linux counts a NUL after the path in the length, other systems
            // do not, and a caller may bind without one. unix(7) gives the
            // rule: the path ends at the first NUL within the length.
            Some(_) => {
                let end = path.iter().position(|&b| b == 0).unwrap_or(len);
                AddrKind::Pathname(path.get(..end).unwrap_or(path))
            }
        }
    }

    const fn c_addr(&self) -> CAddr<'_> {
        CAddr {
            ptr: ptr::from_ref(&self.addr).cast(),
            len: self.len,
            _holder: PhantomData,
        }
    }
}

impl Socket {
    /// Opens a close-on-exec Unix domain socket.
    pub(crate) fn unix(ty: UnixType) -> io::Result<Self> {
        Self::new(libc::AF_UNIX, ty.raw())
    }

    /// Opens a connected pair of close-on-exec Unix domain sockets. macOS
    /// sets the flag after creating them, and, as std, no `SO_NOSIGPIPE`.
    pub(crate) fn unix_pair(ty: UnixType) -> io::Result<(Self, Self)> {
        let mut fds = [0; 2];
        #[cfg(not(target_vendor = "apple"))]
        let ty = ty.raw() | libc::SOCK_CLOEXEC;
        #[cfg(target_vendor = "apple")]
        let ty = ty.raw();
        // SAFETY: `fds` is valid for writes of the two descriptors.
        cvt(unsafe {
            libc::socketpair(libc::AF_UNIX, ty, 0, fds.as_mut_ptr())
        })?;
        // SAFETY: `socketpair` returned two new descriptors that nothing
        // else owns.
        let pair = fds.map(|fd| Self(unsafe { OwnedFd::from_raw_fd(fd) }));
        #[cfg(target_vendor = "apple")]
        for socket in &pair {
            os::set_cloexec(socket.raw())?;
        }
        Ok(pair.into())
    }

    pub(crate) fn bind_unix(&self, addr: &UnixAddr) -> io::Result<()> {
        self.bind(addr.c_addr())
    }

    /// Listens with std's backlog. For a listener bound to a path, -1,
    /// which the kernel caps at its limit, or `SOMAXCONN` on NetBSD and
    /// DragonFly; for one bound to an address (`bind_addr`), -1 on Linux and
    /// 128 elsewhere.
    pub(crate) fn listen_unix(&self, by_path: bool) -> io::Result<()> {
        let backlog = if by_path {
            if cfg!(any(target_os = "netbsd", target_os = "dragonfly")) {
                libc::SOMAXCONN
            } else {
                -1
            }
        } else if cfg!(target_os = "linux") {
            -1
        } else {
            128
        };
        self.listen(backlog)
    }

    /// Connects once: unlike for TCP, std does not retry after a signal.
    pub(crate) fn connect_unix(&self, addr: &UnixAddr) -> io::Result<()> {
        self.connect_once(addr.c_addr())
    }

    /// Accepts a connection, returning its socket and the peer's address.
    pub(crate) fn accept_unix(&self) -> io::Result<(Self, UnixAddr)> {
        let mut storage = SockaddrUn::ZERO;
        let (socket, len) = self.accept_into(&mut storage)?;
        Ok((socket, UnixAddr::from_parts(storage, len)?))
    }

    pub(crate) fn unix_socket_addr(&self) -> io::Result<UnixAddr> {
        let mut storage = SockaddrUn::ZERO;
        let len = self.name_into(libc::getsockname, &mut storage)?;
        UnixAddr::from_parts(storage, len)
    }

    pub(crate) fn unix_peer_addr(&self) -> io::Result<UnixAddr> {
        let mut storage = SockaddrUn::ZERO;
        let len = self.name_into(libc::getpeername, &mut storage)?;
        UnixAddr::from_parts(storage, len)
    }

    /// Receives a datagram, returning its length and sender.
    pub(crate) fn recv_from_unix(
        &self,
        buf: &mut [u8],
    ) -> io::Result<(usize, UnixAddr)> {
        let mut storage = SockaddrUn::ZERO;
        let (n, len) = self.recv_from_into(buf, 0, &mut storage)?;
        Ok((n, UnixAddr::from_parts(storage, len)?))
    }

    /// Sends `buf` as one datagram to `addr`.
    pub(crate) fn send_to_unix(
        &self,
        buf: &[u8],
        addr: &UnixAddr,
    ) -> io::Result<usize> {
        self.send_to_addr(buf, addr.c_addr())
    }
}
