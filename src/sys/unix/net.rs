//! Sockets on top of the BSD socket API.
//!
//! Every socket is opened close-on-exec and owned by an `OwnedFd`. Sends
//! pass `MSG_NOSIGNAL`: `SIGPIPE` keeps its default action, which ends the
//! process, so a write to a closed connection fails with `BrokenPipe`.
//!
//! macOS cannot create sockets close-on-exec: they get the flag right after,
//! as in std, so a child that another thread spawns at that moment inherits
//! them. The sockets litestd opens there also get `SO_NOSIGPIPE`, as in std,
//! which their accepted connections inherit and which spares their other
//! writers the signal too.

mod inet;
#[cfg(feature = "path")]
mod local;
mod lookup;

use core::{
    ffi::c_int,
    marker::PhantomData,
    mem::{self, MaybeUninit},
    ptr,
    time::Duration,
};

use alloc_crate::{string::String, vec::Vec};

#[cfg(feature = "path")]
pub(crate) use self::local::{AddrKind, UnixAddr, UnixType};
pub(crate) use self::lookup::lookup_host;
use crate::{
    io::{self, IoSlice, IoSliceMut},
    net::{
        Shutdown, ZERO_TIMEOUT,
        sockaddr::{SockAddr, Storage},
    },
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    sys::os::{self, cvt, cvt_len, cvt_r, iov_count},
};

/// `AF_INET` as `net::sockaddr` passes it to `sockaddr_head`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a small positive constant"
)]
pub(crate) const AF_INET: u16 = libc::AF_INET as u16;

/// `AF_INET6` as `net::sockaddr` passes it to `sockaddr_head`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a small positive constant"
)]
pub(crate) const AF_INET6: u16 = libc::AF_INET6 as u16;

/// The first two bytes of an IP socket address of `family` that is `len`
/// bytes long: the family, as Linux's 16-bit `sa_family_t`.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) const fn sockaddr_head(family: u16, _len: u8) -> [u8; 2] {
    family.to_ne_bytes()
}

/// The first two bytes of an IP socket address of `family` that is `len`
/// bytes long: the length, which the kernel ignores in the addresses it
/// takes and sets in those it returns, then the family as the 8-bit
/// `sa_family_t` of macOS and the BSDs.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
#[allow(
    clippy::cast_possible_truncation,
    reason = "the families are small constants"
)]
pub(crate) const fn sockaddr_head(family: u16, len: u8) -> [u8; 2] {
    [len, family as u8]
}

/// The family of the IP socket address whose first two bytes are `head`.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) const fn sockaddr_family(head: [u8; 2]) -> u16 {
    u16::from_ne_bytes(head)
}

/// The family of the IP socket address whose first two bytes are `head`:
/// the second byte, as std reads it, whatever length the first one reports.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
#[allow(clippy::cast_lossless, reason = "`From` is not `const`")]
pub(crate) const fn sockaddr_family(head: [u8; 2]) -> u16 {
    head[1] as u16
}

/// A socket address in the C form the kernel reads: the first `len` bytes
/// at `ptr`, initialized and borrowed from their holder for `'a`.
#[derive(Clone, Copy)]
struct CAddr<'a> {
    ptr: *const libc::sockaddr,
    len: libc::socklen_t,
    _holder: PhantomData<&'a [u8]>,
}

impl<'a> CAddr<'a> {
    fn ip(addr: &'a SockAddr) -> Self {
        Self {
            ptr: addr.as_ptr(),
            len: addr.len().into(),
            _holder: PhantomData,
        }
    }
}

/// Memory for a socket address that the kernel writes, lent as a pointer
/// and its size for as long as the `&mut` borrow lasts. It holds plain
/// bytes, so any bytes the kernel writes leave it valid.
trait AddrRoom {
    fn as_mut_c(&mut self) -> (*mut libc::sockaddr, libc::socklen_t);
}

impl AddrRoom for Storage {
    fn as_mut_c(&mut self) -> (*mut libc::sockaddr, libc::socklen_t) {
        (self.as_mut_ptr(), Self::LEN.into())
    }
}

/// `getsockname` or `getpeername`.
type NameFn = unsafe extern "C" fn(
    c_int,
    *mut libc::sockaddr,
    *mut libc::socklen_t,
) -> c_int;

/// The C types of the socket option values litestd passes. Each is plain
/// integers without padding, for which every byte pattern is valid, so the
/// kernel reads only initialized bytes and may write any.
trait OptionValue: Copy {
    const ZERO: Self;
}

impl OptionValue for c_int {
    const ZERO: Self = 0;
}

impl OptionValue for libc::timeval {
    const ZERO: Self = Self {
        tv_sec: 0,
        tv_usec: 0,
    };
}

impl OptionValue for libc::ip_mreq {
    const ZERO: Self = Self {
        imr_multiaddr: libc::in_addr { s_addr: 0 },
        imr_interface: libc::in_addr { s_addr: 0 },
    };
}

impl OptionValue for libc::ipv6_mreq {
    const ZERO: Self = Self {
        ipv6mr_multiaddr: libc::in6_addr { s6_addr: [0; 16] },
        ipv6mr_interface: 0,
    };
}

/// An open socket.
pub(crate) struct Socket(OwnedFd);

impl Socket {
    /// Opens a close-on-exec socket, which macOS flags after creating it. It
    /// has `SO_NOSIGPIPE` on macOS, FreeBSD, NetBSD and DragonFly, as in std;
    /// `ty` may add `SOCK_NONBLOCK` where the call takes it.
    fn new(family: c_int, ty: c_int) -> io::Result<Self> {
        #[cfg(not(target_vendor = "apple"))]
        let ty = ty | libc::SOCK_CLOEXEC;
        // SAFETY: `socket` touches no memory.
        let fd = cvt(unsafe { libc::socket(family, ty, 0) })?;
        // SAFETY: `socket` returned a new descriptor that nothing else owns.
        let socket = Self(unsafe { OwnedFd::from_raw_fd(fd) });
        #[cfg(target_vendor = "apple")]
        os::set_cloexec(socket.raw())?;
        #[cfg(any(
            target_vendor = "apple",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        socket.set_flag(libc::SOL_SOCKET, libc::SO_NOSIGPIPE, true)?;
        Ok(socket)
    }

    /// Opens a close-on-exec socket in nonblocking mode.
    #[cfg(not(target_vendor = "apple"))]
    fn new_nonblocking(family: c_int, ty: c_int) -> io::Result<Self> {
        Self::new(family, ty | libc::SOCK_NONBLOCK)
    }

    /// Opens a close-on-exec socket in nonblocking mode, which macOS sets
    /// with a second call.
    #[cfg(target_vendor = "apple")]
    fn new_nonblocking(family: c_int, ty: c_int) -> io::Result<Self> {
        let socket = Self::new(family, ty)?;
        socket.set_nonblocking(true)?;
        Ok(socket)
    }

    pub(crate) const fn from_inner(fd: OwnedFd) -> Self {
        Self(fd)
    }

    pub(crate) const fn as_inner(&self) -> &OwnedFd {
        &self.0
    }

    pub(crate) fn into_inner(self) -> OwnedFd {
        self.0
    }

    fn raw(&self) -> RawFd {
        self.0.as_raw_fd()
    }

    /// The field std's `Debug` output shows for the socket: name and value.
    pub(crate) fn raw_debug(&self) -> (&'static str, RawFd) {
        ("fd", self.raw())
    }

    fn bind(&self, addr: CAddr<'_>) -> io::Result<()> {
        // SAFETY: `addr` points to `addr.len` initialized bytes.
        cvt(unsafe { libc::bind(self.raw(), addr.ptr, addr.len) }).map(drop)
    }

    fn listen(&self, backlog: c_int) -> io::Result<()> {
        // SAFETY: `listen` touches no memory.
        cvt(unsafe { libc::listen(self.raw(), backlog) }).map(drop)
    }

    /// Calls `connect` once. A signal that interrupts a blocking connect
    /// makes it fail with `Interrupted`, while the connection goes on.
    fn connect_once(&self, addr: CAddr<'_>) -> io::Result<()> {
        // SAFETY: `addr` points to `addr.len` initialized bytes.
        cvt(unsafe { libc::connect(self.raw(), addr.ptr, addr.len) }).map(drop)
    }

    /// Accepts a connection close-on-exec, storing the peer's address in
    /// `addr`; returns the length reported, larger if it did not fit.
    fn accept_into(
        &self,
        addr: &mut impl AddrRoom,
    ) -> io::Result<(Self, usize)> {
        let (ptr, room) = addr.as_mut_c();
        let mut len = room;
        let fd = cvt_r(|| {
            len = room;
            // SAFETY: `ptr` is valid for writes of `len` bytes, the most the
            // kernel writes, and `len` for reads and writes.
            #[cfg(not(target_vendor = "apple"))]
            let fd = unsafe {
                libc::accept4(self.raw(), ptr, &raw mut len, libc::SOCK_CLOEXEC)
            };
            // SAFETY: as above.
            #[cfg(target_vendor = "apple")]
            let fd = unsafe { libc::accept(self.raw(), ptr, &raw mut len) };
            fd
        })?;
        // SAFETY: the call returned a new descriptor that nothing else owns.
        let socket = Self(unsafe { OwnedFd::from_raw_fd(fd) });
        // macOS has no `accept4`; the connection inherits `SO_NOSIGPIPE`.
        #[cfg(target_vendor = "apple")]
        os::set_cloexec(socket.raw())?;
        Ok((socket, len as usize))
    }

    /// Stores the socket's own or its peer's address, as `name` says, in
    /// `addr` and returns the length the kernel reported for it.
    fn name_into(
        &self,
        name: NameFn,
        addr: &mut impl AddrRoom,
    ) -> io::Result<usize> {
        let (ptr, mut len) = addr.as_mut_c();
        // SAFETY: `name` is `getsockname` or `getpeername`; `ptr` is valid
        // for writes of `len` bytes, the most the kernel writes.
        cvt(unsafe { name(self.raw(), ptr, &raw mut len) })?;
        Ok(len as usize)
    }

    /// Receives into `buf`, storing the sender's address in `addr`, and
    /// returns the byte count and the address length the kernel reported.
    fn recv_from_into(
        &self,
        buf: &mut [u8],
        flags: c_int,
        addr: &mut impl AddrRoom,
    ) -> io::Result<(usize, usize)> {
        let (ptr, mut len) = addr.as_mut_c();
        // SAFETY: `buf` is valid for writes of its length and `ptr` of `len`
        // bytes, the most the kernel writes to each.
        let n = cvt_len(unsafe {
            libc::recvfrom(
                self.raw(),
                buf.as_mut_ptr().cast(),
                buf.len().min(os::MAX_LEN),
                flags,
                ptr,
                &raw mut len,
            )
        })?;
        Ok((n, len as usize))
    }

    /// Sends `buf` as one datagram to `addr`.
    fn send_to_addr(&self, buf: &[u8], addr: CAddr<'_>) -> io::Result<usize> {
        // SAFETY: `buf` is valid for reads of its length, and `addr` points
        // to `addr.len` initialized bytes.
        cvt_len(unsafe {
            libc::sendto(
                self.raw(),
                buf.as_ptr().cast(),
                buf.len(),
                libc::MSG_NOSIGNAL,
                addr.ptr,
                addr.len,
            )
        })
    }

    pub(crate) fn duplicate(&self) -> io::Result<Self> {
        self.0.try_clone().map(Self)
    }

    /// Receives into `buf`, which only the kernel writes: it initializes the
    /// bytes it reports received and touches no others.
    fn recv(
        &self,
        buf: &mut [MaybeUninit<u8>],
        flags: c_int,
    ) -> io::Result<usize> {
        let len = buf.len().min(os::MAX_LEN);
        // SAFETY: `buf` is valid for writes of `len` bytes.
        cvt_len(unsafe {
            libc::recv(self.raw(), buf.as_mut_ptr().cast(), len, flags)
        })
    }

    pub(crate) fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: `recv` lends the buffer to the kernel alone.
        self.recv(unsafe { io::as_uninit(buf) }, 0)
    }

    pub(crate) fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: `recv` lends the buffer to the kernel alone.
        self.recv(unsafe { io::as_uninit(buf) }, libc::MSG_PEEK)
    }

    /// Reads to the end of the stream straight into the spare capacity of
    /// `buf`, which is never zeroed first.
    pub(crate) fn read_to_end(&self, buf: &mut Vec<u8>) -> io::Result<usize> {
        // SAFETY: `recv` initializes what it reports, nothing else.
        unsafe {
            io::read_to_end_uninit(buf, None, |spare| self.recv(spare, 0))
        }
    }

    pub(crate) fn read_to_string(&self, buf: &mut String) -> io::Result<usize> {
        // SAFETY: `read_to_end` only appends to the vector.
        unsafe { io::append_to_string(buf, |bytes| self.read_to_end(bytes)) }
    }

    pub(crate) fn read_vectored(
        &self,
        bufs: &mut [IoSliceMut<'_>],
    ) -> io::Result<usize> {
        os::read_vectored(self.raw(), bufs)
    }

    /// Sends from `buf`, as much of it as one call takes: on macOS at most
    /// `INT_MAX` bytes, the most its kernel accepts.
    pub(crate) fn write(&self, buf: &[u8]) -> io::Result<usize> {
        let len = buf.len().min(os::MAX_LEN);
        // SAFETY: `buf` is valid for reads of `len` bytes.
        cvt_len(unsafe {
            libc::send(self.raw(), buf.as_ptr().cast(), len, libc::MSG_NOSIGNAL)
        })
    }

    /// Writes with `sendmsg` rather than `writev`, which cannot pass
    /// `MSG_NOSIGNAL`.
    pub(crate) fn write_vectored(
        &self,
        bufs: &[IoSlice<'_>],
    ) -> io::Result<usize> {
        // SAFETY: `msghdr` holds only integers and raw pointers, for which
        // all zeros is valid: no address, no control data, no flags.
        let mut msg: libc::msghdr = unsafe { mem::zeroed() };
        // `sendmsg` does not write through the pointer.
        msg.msg_iov = bufs.as_ptr().cast_mut().cast();
        // A small positive count, for a field that is `size_t` or `int` by C
        // library.
        #[allow(clippy::cast_sign_loss, clippy::unnecessary_cast)]
        let iovlen = iov_count(bufs.len()) as _;
        msg.msg_iovlen = iovlen;
        // SAFETY: `IoSlice` has the layout of `iovec`, and `msg` points only
        // to the first `msg_iovlen` of `bufs`, all valid for reads.
        cvt_len(unsafe {
            libc::sendmsg(self.raw(), &raw const msg, libc::MSG_NOSIGNAL)
        })
    }

    pub(crate) fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        let how = match how {
            Shutdown::Read => libc::SHUT_RD,
            Shutdown::Write => libc::SHUT_WR,
            Shutdown::Both => libc::SHUT_RDWR,
        };
        // SAFETY: `shutdown` touches no memory.
        cvt(unsafe { libc::shutdown(self.raw(), how) }).map(drop)
    }

    /// Sets `O_NONBLOCK` with one `ioctl`, as std does, rather than the two
    /// `fcntl` calls that read and write the whole flag set.
    pub(crate) fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        let mut on = c_int::from(nonblocking);
        // SAFETY: `FIONBIO` reads the `int` that `on` holds.
        cvt(unsafe { libc::ioctl(self.raw(), libc::FIONBIO, &raw mut on) })
            .map(drop)
    }

    fn setsockopt<T: OptionValue>(
        &self,
        level: c_int,
        name: c_int,
        value: T,
    ) -> io::Result<()> {
        // The option types are a few bytes long.
        #[allow(clippy::cast_possible_truncation)]
        let len = size_of::<T>() as libc::socklen_t;
        // SAFETY: `value` holds `len` initialized bytes.
        cvt(unsafe {
            libc::setsockopt(
                self.raw(),
                level,
                name,
                ptr::from_ref(&value).cast(),
                len,
            )
        })
        .map(drop)
    }

    fn getsockopt<T: OptionValue>(
        &self,
        level: c_int,
        name: c_int,
    ) -> io::Result<T> {
        let mut value = T::ZERO;
        // The option types are a few bytes long.
        #[allow(clippy::cast_possible_truncation)]
        let size = size_of::<T>() as libc::socklen_t;
        let mut len = size;
        // SAFETY: `value` is valid for writes of `len` bytes, the most the
        // kernel writes, and any bytes it writes leave a valid `T`.
        cvt(unsafe {
            libc::getsockopt(
                self.raw(),
                level,
                name,
                ptr::from_mut(&mut value).cast(),
                &raw mut len,
            )
        })?;
        // The kernel writes its own layout of the option, which the libc
        // crate's `T` matches; any other size would be misread.
        if len != size {
            return Err(io::const_error!(
                io::ErrorKind::InvalidData,
                "unexpected size of a socket option",
            ));
        }
        Ok(value)
    }

    fn set_flag(&self, level: c_int, name: c_int, on: bool) -> io::Result<()> {
        self.setsockopt(level, name, c_int::from(on))
    }

    fn flag(&self, level: c_int, name: c_int) -> io::Result<bool> {
        Ok(self.getsockopt::<c_int>(level, name)? != 0)
    }

    /// Sets the timeout `kind`. Like std, it rounds a timeout below a
    /// microsecond up to one, which Linux rounds up to a clock tick and
    /// macOS keeps as it is, and caps the seconds at `time_t::MAX`. Unlike
    /// std, it sets no microseconds there with a 32-bit `time_t`: the kernel
    /// would round them up into a second that it cannot report back.
    // libc deprecates `time_t` and `suseconds_t` on musl ahead of its 64-bit
    // switch, while `timeval` still uses them.
    #[allow(deprecated)]
    fn set_timeout(
        &self,
        dur: Option<Duration>,
        kind: c_int,
    ) -> io::Result<()> {
        let tv = match dur {
            None => libc::timeval::ZERO,
            Some(dur) if dur.is_zero() => return Err(ZERO_TIMEOUT),
            Some(dur) => {
                let secs = libc::time_t::try_from(dur.as_secs())
                    .unwrap_or(libc::time_t::MAX);
                // Below one million, which fits every `suseconds_t`, some
                // of which cannot hold every `u32`.
                #[allow(clippy::cast_lossless, clippy::cast_possible_wrap)]
                let micros = dur.subsec_micros() as libc::suseconds_t;
                libc::timeval {
                    tv_sec: secs,
                    tv_usec: match secs {
                        0 => micros.max(1),
                        libc::time_t::MAX if size_of::<libc::time_t>() == 4 => {
                            0
                        }
                        _ => micros,
                    },
                }
            }
        };
        self.setsockopt(libc::SOL_SOCKET, kind, tv)
    }

    fn timeout(&self, kind: c_int) -> io::Result<Option<Duration>> {
        let tv: libc::timeval = self.getsockopt(libc::SOL_SOCKET, kind)?;
        if tv.tv_sec == 0 && tv.tv_usec == 0 {
            return Ok(None);
        }
        // Capping the microseconds drops the carry path of `Duration::new`.
        let micros = u32::try_from(tv.tv_usec).unwrap_or(0).min(999_999);
        Ok(Some(Duration::new(timeout_secs(tv.tv_sec), micros * 1000)))
    }

    pub(crate) fn set_read_timeout(
        &self,
        dur: Option<Duration>,
    ) -> io::Result<()> {
        self.set_timeout(dur, libc::SO_RCVTIMEO)
    }

    pub(crate) fn set_write_timeout(
        &self,
        dur: Option<Duration>,
    ) -> io::Result<()> {
        self.set_timeout(dur, libc::SO_SNDTIMEO)
    }

    pub(crate) fn read_timeout(&self) -> io::Result<Option<Duration>> {
        self.timeout(libc::SO_RCVTIMEO)
    }

    pub(crate) fn write_timeout(&self) -> io::Result<Option<Duration>> {
        self.timeout(libc::SO_SNDTIMEO)
    }

    pub(crate) fn take_error(&self) -> io::Result<Option<io::Error>> {
        let code: c_int = self.getsockopt(libc::SOL_SOCKET, libc::SO_ERROR)?;
        Ok((code != 0).then(|| io::Error::from_raw_os_error(code)))
    }
}

/// The seconds of a timeout that the kernel reports. It stores no negative
/// timeout, but truncates a 32-bit `time_t`, which then reads as negative
/// from 2^31 seconds on: read as unsigned, it is exact below 2^32 seconds,
/// beyond anything a 32-bit `time_t` sets.
// libc deprecates `time_t` on musl ahead of its 64-bit switch. The casts
// reinterpret the 32 bits of a 32-bit `time_t`; the branch is dead for a
// wider one.
#[allow(deprecated, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn timeout_secs(secs: libc::time_t) -> u64 {
    if size_of::<libc::time_t>() == 4 {
        u64::from(secs as u32)
    } else {
        u64::try_from(secs).unwrap_or(0)
    }
}
