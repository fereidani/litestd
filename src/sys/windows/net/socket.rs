//! [`Socket`], an owned Winsock socket, and the operations of every kind.

use core::{mem::MaybeUninit, ptr, time::Duration};

use alloc_crate::{string::String, vec::Vec};
use windows_sys::Win32::Networking::WinSock::{
    self as ws, FIONBIO, MSG_PEEK, SD_BOTH, SD_RECEIVE, SD_SEND, SO_ERROR,
    SO_RCVTIMEO, SO_SNDTIMEO, SOCKET, SOL_SOCKET, WSABUF, WSAESHUTDOWN,
    WSAGetLastError,
};

use super::{
    super::os::{handle::read_to_end_io, timeout_ms},
    cvt, cvt_len, to_socket, wsa_socket,
};
use crate::{
    io::{self, IoSlice, IoSliceMut},
    net::{Shutdown, ZERO_TIMEOUT},
    os::windows::io::{AsRawSocket, OwnedSocket, RawSocket},
};

/// The most buffers one vectored transfer passes to Winsock; the buffers
/// after them wait for the next call, as after any partial transfer.
const MAX_BUFS: usize = 64;

/// An open Winsock socket, which child processes do not inherit.
pub(crate) struct Socket(OwnedSocket);

/// Clamps a length to the `i32` that Winsock calls take.
pub(super) fn clamp(len: usize) -> i32 {
    i32::try_from(len).unwrap_or(i32::MAX)
}

/// Fills `out` with the non-empty buffers of a vectored transfer, in order,
/// as many as fit, and returns how many it filled and their total length.
///
/// Skipping empty buffers keeps a run of them from crowding out data; only
/// if all are empty does one go to Winsock. The total stays within
/// `u32::MAX`, the most Winsock can report: a buffer beyond it is cut short
/// and ends the list, so the transfer covers a prefix without gaps.
fn fill_bufs(
    out: &mut [MaybeUninit<WSABUF>; MAX_BUFS],
    bufs: impl Iterator<Item = (*mut u8, usize)>,
) -> (u32, u32) {
    let (mut count, mut total) = (0, 0u32);
    let mut empty = None;
    // Bounded by the number of buffers.
    for (buf, len) in bufs {
        if len == 0 {
            empty.get_or_insert(buf);
            continue;
        }
        let Some(slot) = out.get_mut(count) else {
            break;
        };
        let part = u32::try_from(len).unwrap_or(u32::MAX).min(u32::MAX - total);
        slot.write(WSABUF { len: part, buf });
        count += 1;
        total += part;
        if usize::try_from(part).ok() != Some(len) {
            break;
        }
    }
    if let (0, Some(buf)) = (count, empty) {
        out[0].write(WSABUF { len: 0, buf });
        count = 1;
    }
    // At most `MAX_BUFS`.
    #[allow(clippy::cast_possible_truncation)]
    let count = count as u32;
    (count, total)
}

/// Handles a failed receive: a socket shut down for reading is at the end
/// of its stream, as on Unix.
#[cold]
pub(super) fn shutdown_as_eof() -> io::Result<usize> {
    // SAFETY: `WSAGetLastError` has no preconditions.
    match unsafe { WSAGetLastError() } {
        WSAESHUTDOWN => Ok(0),
        code => Err(io::Error::from_raw_os_error(code)),
    }
}

impl Socket {
    /// Creates a socket of `family` and type `ty`; Winsock must run.
    pub(super) fn new(family: u16, ty: i32) -> io::Result<Self> {
        wsa_socket(family.into(), ty, 0, None).map(Self)
    }

    pub(crate) const fn from_inner(owned: OwnedSocket) -> Self {
        Self(owned)
    }

    pub(crate) const fn as_inner(&self) -> &OwnedSocket {
        &self.0
    }

    pub(crate) fn into_inner(self) -> OwnedSocket {
        self.0
    }

    #[inline]
    pub(super) fn raw(&self) -> SOCKET {
        to_socket(self.0.as_raw_socket())
    }

    /// The field name and value that std's `Debug` output shows.
    pub(crate) fn raw_debug(&self) -> (&'static str, RawSocket) {
        ("socket", self.0.as_raw_socket())
    }

    pub(crate) fn duplicate(&self) -> io::Result<Self> {
        self.0.try_clone().map(Self)
    }

    /// Receives into `buf`, which only Winsock writes: it initializes the
    /// bytes it reports received and touches no others. A socket shut down
    /// for reading reads as the end of the stream, as on Unix.
    fn recv(
        &self,
        buf: &mut [MaybeUninit<u8>],
        flags: i32,
    ) -> io::Result<usize> {
        // SAFETY: the socket is open, and `buf` is valid for writes of the
        // length passed, which is at most its own.
        let result = unsafe {
            ws::recv(
                self.raw(),
                buf.as_mut_ptr().cast(),
                clamp(buf.len()),
                flags,
            )
        };
        usize::try_from(result)
            .map_or_else(|_| shutdown_as_eof(), |n| Ok(n.min(buf.len())))
    }

    fn read_uninit(&self, buf: &mut [MaybeUninit<u8>]) -> io::Result<usize> {
        self.recv(buf, 0)
    }

    pub(crate) fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: `recv` lends the buffer to Winsock alone.
        self.read_uninit(unsafe { io::as_uninit(buf) })
    }

    pub(crate) fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: `recv` lends the buffer to Winsock alone.
        self.recv(unsafe { io::as_uninit(buf) }, MSG_PEEK)
    }

    read_to_end_io!();

    pub(crate) fn read_vectored(
        &self,
        bufs: &mut [IoSliceMut<'_>],
    ) -> io::Result<usize> {
        let mut out = [MaybeUninit::uninit(); MAX_BUFS];
        let bufs = bufs.iter_mut().map(|b| (b.as_mut_ptr(), b.len()));
        let (count, total) = fill_bufs(&mut out, bufs);
        let (mut read, mut flags) = (0, 0);
        // SAFETY: the socket is open; `out` starts with `count` initialized
        // entries for writable parts of `bufs`; `read` and `flags` are
        // writable; without an `OVERLAPPED` the call is synchronous.
        let result = unsafe {
            ws::WSARecv(
                self.raw(),
                out.as_ptr().cast(),
                count,
                &raw mut read,
                &raw mut flags,
                ptr::null_mut(),
                None,
            )
        };
        if result == 0 {
            Ok(read.min(total) as usize)
        } else {
            shutdown_as_eof()
        }
    }

    /// Sends from `buf`, at most `i32::MAX` bytes of it, as std does.
    pub(crate) fn write(&self, buf: &[u8]) -> io::Result<usize> {
        // SAFETY: the socket is open, and `buf` is valid for reads of the
        // length passed, which is at most its own.
        cvt_len(unsafe {
            ws::send(self.raw(), buf.as_ptr(), clamp(buf.len()), 0)
        })
    }

    pub(crate) fn write_vectored(
        &self,
        bufs: &[IoSlice<'_>],
    ) -> io::Result<usize> {
        let mut out = [MaybeUninit::uninit(); MAX_BUFS];
        // `WSASend` does not write through the mutable pointer of `WSABUF`.
        let bufs = bufs.iter().map(|b| (b.as_ptr().cast_mut(), b.len()));
        let (count, total) = fill_bufs(&mut out, bufs);
        let mut sent = 0;
        // SAFETY: the socket is open; `out` starts with `count` initialized
        // entries for readable parts of `bufs`; `sent` is writable; without
        // an `OVERLAPPED` the call is synchronous.
        cvt(unsafe {
            ws::WSASend(
                self.raw(),
                out.as_ptr().cast(),
                count,
                &raw mut sent,
                0,
                ptr::null_mut(),
                None,
            )
        })?;
        Ok(sent.min(total) as usize)
    }

    pub(crate) fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        let how = match how {
            Shutdown::Read => SD_RECEIVE,
            Shutdown::Write => SD_SEND,
            Shutdown::Both => SD_BOTH,
        };
        // SAFETY: the socket is open; `shutdown` takes no pointers.
        cvt(unsafe { ws::shutdown(self.raw(), how) })
    }

    pub(crate) fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        let mut arg = u32::from(nonblocking);
        // SAFETY: the socket is open, and `arg` is valid for reads and
        // writes of the `u32` that `FIONBIO` takes.
        cvt(unsafe { ws::ioctlsocket(self.raw(), FIONBIO, &raw mut arg) })
    }

    /// Sets option `name` at `level` to `value`, of the option's C type.
    pub(super) fn set_option<T: Copy>(
        &self,
        level: i32,
        name: i32,
        value: T,
    ) -> io::Result<()> {
        // The option types are a few bytes long.
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let len = size_of::<T>() as i32;
        let value = ptr::from_ref(&value).cast();
        // SAFETY: the socket is open, and `value` is valid for reads of
        // `len` bytes during the call.
        cvt(unsafe { ws::setsockopt(self.raw(), level, name, value, len) })
    }

    /// Reads option `name` at `level`, at most four bytes long. The value
    /// starts zeroed: Winsock writes fewer bytes for some options, such as
    /// one for `TCP_NODELAY` on some versions.
    pub(super) fn option(&self, level: i32, name: i32) -> io::Result<[u8; 4]> {
        let mut value = [0; 4];
        let mut len = 4;
        let ptr = value.as_mut_ptr();
        // SAFETY: the socket is open, `value` is valid for writes of `len`
        // bytes and `len` for writes of an `i32`.
        cvt(unsafe {
            ws::getsockopt(self.raw(), level, name, ptr, &raw mut len)
        })?;
        Ok(value)
    }

    /// Sets `SO_RCVTIMEO` or `SO_SNDTIMEO`, in milliseconds rounded up.
    fn set_timeout(&self, dur: Option<Duration>, name: i32) -> io::Result<()> {
        let ms = match dur {
            None => 0,
            Some(dur) if dur.is_zero() => return Err(ZERO_TIMEOUT),
            // `INFINITE` for those beyond `u32::MAX`, as std does.
            Some(dur) => timeout_ms(dur, u32::MAX),
        };
        self.set_option(SOL_SOCKET, name, ms)
    }

    fn timeout(&self, name: i32) -> io::Result<Option<Duration>> {
        let ms = u32::from_ne_bytes(self.option(SOL_SOCKET, name)?);
        Ok((ms != 0).then(|| Duration::from_millis(ms.into())))
    }

    pub(crate) fn set_read_timeout(
        &self,
        dur: Option<Duration>,
    ) -> io::Result<()> {
        self.set_timeout(dur, SO_RCVTIMEO)
    }

    pub(crate) fn set_write_timeout(
        &self,
        dur: Option<Duration>,
    ) -> io::Result<()> {
        self.set_timeout(dur, SO_SNDTIMEO)
    }

    pub(crate) fn read_timeout(&self) -> io::Result<Option<Duration>> {
        self.timeout(SO_RCVTIMEO)
    }

    pub(crate) fn write_timeout(&self) -> io::Result<Option<Duration>> {
        self.timeout(SO_SNDTIMEO)
    }

    pub(crate) fn take_error(&self) -> io::Result<Option<io::Error>> {
        let code = i32::from_ne_bytes(self.option(SOL_SOCKET, SO_ERROR)?);
        Ok((code != 0).then(|| io::Error::from_raw_os_error(code)))
    }
}
