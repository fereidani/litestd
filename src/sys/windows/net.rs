//! Sockets on top of Winsock.
//!
//! Winsock starts on the first socket or lookup and is never cleaned up; the
//! process's exit releases it. Sockets are overlapped and not inheritable, as
//! in std, but transfers are Winsock calls without an `OVERLAPPED`, which are
//! synchronous on any socket: the kernel never writes to a buffer, status or
//! address after the call that lent it returned. No socket goes through
//! `ReadFile` or the native file API.

mod inet;
mod lookup;
mod socket;

use core::{
    cell::UnsafeCell,
    mem::{MaybeUninit, offset_of},
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};

pub(crate) use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
use windows_sys::Win32::{
    Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation},
    Networking::WinSock::{
        INVALID_SOCKET, SOCKADDR_IN, SOCKADDR_IN6, SOCKADDR_STORAGE, SOCKET,
        SOCKET_ERROR, WSA_FLAG_NO_HANDLE_INHERIT, WSA_FLAG_OVERLAPPED, WSADATA,
        WSADuplicateSocketW, WSAEINVAL, WSAEPROTOTYPE, WSAGetLastError,
        WSAPROTOCOL_INFOW, WSASocketW, WSAStartup, closesocket,
    },
    System::Threading::{
        AcquireSRWLockExclusive, GetCurrentProcessId, ReleaseSRWLockExclusive,
        SRWLOCK, SRWLOCK_INIT,
    },
};

pub(crate) use self::{lookup::lookup_host, socket::Socket};
use crate::{
    io,
    net::sockaddr::{Storage, V4, V6},
    os::windows::io::{
        AsRawSocket, BorrowedSocket, FromRawSocket, OwnedSocket, RawSocket,
    },
};

// The mirrors of `net::sockaddr` have the layout of Winsock's structures.
const _: () = {
    assert!(size_of::<V4>() == size_of::<SOCKADDR_IN>());
    assert!(align_of::<V4>() >= align_of::<SOCKADDR_IN>());
    assert!(offset_of!(V4, port) == offset_of!(SOCKADDR_IN, sin_port));
    assert!(offset_of!(V4, addr) == offset_of!(SOCKADDR_IN, sin_addr));
    assert!(size_of::<V6>() == size_of::<SOCKADDR_IN6>());
    assert!(align_of::<V6>() >= align_of::<SOCKADDR_IN6>());
    assert!(offset_of!(V6, port) == offset_of!(SOCKADDR_IN6, sin6_port));
    assert!(
        offset_of!(V6, flowinfo) == offset_of!(SOCKADDR_IN6, sin6_flowinfo)
    );
    assert!(offset_of!(V6, addr) == offset_of!(SOCKADDR_IN6, sin6_addr));
    assert!(offset_of!(V6, scope_id) == offset_of!(SOCKADDR_IN6, Anonymous));
    assert!(size_of::<Storage>() == size_of::<SOCKADDR_STORAGE>());
    assert!(align_of::<Storage>() >= align_of::<SOCKADDR_STORAGE>());
    assert!(offset_of!(V4, head) == offset_of!(SOCKADDR_IN, sin_family));
    assert!(offset_of!(V6, head) == offset_of!(SOCKADDR_IN6, sin6_family));
};

/// The first two bytes of an IP socket address of `family`: the 16-bit
/// family.
pub(crate) const fn sockaddr_head(family: u16, _len: u8) -> [u8; 2] {
    family.to_ne_bytes()
}

/// The family of the IP socket address whose first two bytes are `head`.
pub(crate) const fn sockaddr_family(head: [u8; 2]) -> u16 {
    u16::from_ne_bytes(head)
}

/// Whether `WSAStartup` succeeded. Set once, never cleared.
static STARTED: AtomicBool = AtomicBool::new(false);

/// Serializes the calls to `WSAStartup`, which is not thread-safe on Wine.
static STARTUP_LOCK: StartupLock = StartupLock(UnsafeCell::new(SRWLOCK_INIT));

/// An SRW lock that can live in a `static`.
struct StartupLock(UnsafeCell<SRWLOCK>);

// SAFETY: the lock is only used through the SRW lock functions, which are
// made to be called on the same lock from any number of threads.
unsafe impl Sync for StartupLock {}

/// Starts Winsock unless it runs already, which costs one atomic load.
///
/// # Errors
///
/// Returns the error of `WSAStartup`; the next call tries again.
#[inline]
pub(crate) fn init() -> io::Result<()> {
    // Acquire: pairs with the release store in `startup`, so that the
    // start of Winsock happens before the socket calls that follow.
    if STARTED.load(Ordering::Acquire) {
        Ok(())
    } else {
        startup()
    }
}

/// Calls `WSAStartup` for version 2.2, unless another thread did.
#[cold]
fn startup() -> io::Result<()> {
    let lock = STARTUP_LOCK.0.get();
    // SAFETY: `lock` is a valid SRW lock for the life of the process; only
    // this function takes it, and releases it before returning.
    unsafe { AcquireSRWLockExclusive(lock) };
    let mut code = 0;
    // Relaxed: the lock orders this load after the store of the thread
    // that held it before.
    if !STARTED.load(Ordering::Relaxed) {
        let mut data = MaybeUninit::<WSADATA>::uninit();
        // SAFETY: `data` is valid for writes of the `WSADATA` it fills in.
        code = unsafe { WSAStartup(0x0202, data.as_mut_ptr()) };
        if code == 0 {
            // Release: see `init`.
            STARTED.store(true, Ordering::Release);
        }
    }
    // SAFETY: this thread acquired the lock above.
    unsafe { ReleaseSRWLockExclusive(lock) };
    // `WSAStartup` returns its error rather than setting the last error.
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code))
    }
}

/// Returns the calling thread's last Winsock error.
#[cold]
fn last_error() -> io::Error {
    // SAFETY: `WSAGetLastError` has no preconditions.
    io::Error::from_raw_os_error(unsafe { WSAGetLastError() })
}

/// Converts the result of a Winsock call that returns zero on success.
fn cvt(result: i32) -> io::Result<()> {
    if result == SOCKET_ERROR {
        Err(last_error())
    } else {
        Ok(())
    }
}

/// Converts the result of a Winsock call that returns a count on success
/// and `SOCKET_ERROR`, the only negative result, on failure.
fn cvt_len(result: i32) -> io::Result<usize> {
    usize::try_from(result).map_err(|_| last_error())
}

/// Converts a `RawSocket` to a `SOCKET`, which has the same width.
#[allow(clippy::cast_possible_truncation, reason = "same width")]
const fn to_socket(raw: RawSocket) -> SOCKET {
    raw as SOCKET
}

/// Takes ownership of a socket that Winsock returned.
///
/// # Safety
///
/// `socket` must be an open socket that nothing else owns.
#[allow(clippy::cast_possible_truncation, reason = "same width")]
unsafe fn owned(socket: SOCKET) -> OwnedSocket {
    // SAFETY: the caller hands over an open socket that nothing else owns,
    // which is never `INVALID_SOCKET`; the cast keeps its value.
    unsafe { OwnedSocket::from_raw_socket(socket as RawSocket) }
}

/// Creates an overlapped, non-inheritable socket, as std does; `info`
/// describes the socket to duplicate, if any.
fn wsa_socket(
    family: i32,
    ty: i32,
    protocol: i32,
    info: Option<&WSAPROTOCOL_INFOW>,
) -> io::Result<OwnedSocket> {
    let info = info.map_or(ptr::null(), ptr::from_ref);
    let flags = WSA_FLAG_OVERLAPPED | WSA_FLAG_NO_HANDLE_INHERIT;
    // SAFETY: `info` is null or points to a protocol info structure that
    // stays valid during the call; the other arguments are plain values.
    let raw = unsafe { WSASocketW(family, ty, protocol, info, 0, flags) };
    if raw == INVALID_SOCKET {
        return wsa_socket_fallback(family, ty, protocol, info);
    }
    // SAFETY: `WSASocketW` returned a new socket that nothing else owns.
    Ok(unsafe { owned(raw) })
}

/// Retries [`wsa_socket`] for providers that reject
/// `WSA_FLAG_NO_HANDLE_INHERIT`, clearing the inherit flag after, as std does.
#[cold]
fn wsa_socket_fallback(
    family: i32,
    ty: i32,
    protocol: i32,
    info: *const WSAPROTOCOL_INFOW,
) -> io::Result<OwnedSocket> {
    // SAFETY: `WSAGetLastError` has no preconditions.
    let code = unsafe { WSAGetLastError() };
    if code != WSAEPROTOTYPE && code != WSAEINVAL {
        return Err(io::Error::from_raw_os_error(code));
    }
    // SAFETY: as in `wsa_socket`.
    let raw = unsafe {
        WSASocketW(family, ty, protocol, info, 0, WSA_FLAG_OVERLAPPED)
    };
    if raw == INVALID_SOCKET {
        return Err(last_error());
    }
    // SAFETY: `WSASocketW` returned a new socket that nothing else owns.
    let owned = unsafe { owned(raw) };
    // Dropping `owned` on failure closes the socket.
    clear_inherit(&owned)?;
    Ok(owned)
}

/// Makes `socket` not inheritable.
fn clear_inherit(socket: &OwnedSocket) -> io::Result<()> {
    // A socket is a kernel handle, an integer rather than an address.
    let handle = ptr::without_provenance_mut(to_socket(socket.as_raw_socket()));
    // SAFETY: the handle is open; changing its flags touches no memory.
    if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Closes a socket, ignoring errors as std does.
///
/// # Safety
///
/// The caller must own `raw` and not use it afterwards.
pub(crate) unsafe fn close(raw: RawSocket) {
    // SAFETY: the caller gives up the open socket it owns.
    unsafe { closesocket(to_socket(raw)) };
}

/// Duplicates a socket for this process, as std does, but not inheritable:
/// Windows can ignore `WSA_FLAG_NO_HANDLE_INHERIT` for a socket made from a
/// duplicate's description.
pub(crate) fn duplicate(
    borrowed: BorrowedSocket<'_>,
) -> io::Result<OwnedSocket> {
    let mut info = WSAPROTOCOL_INFOW::default();
    // SAFETY: the socket is open while borrowed, `info` is valid for writes,
    // and `GetCurrentProcessId` has no preconditions.
    cvt(unsafe {
        WSADuplicateSocketW(
            to_socket(borrowed.as_raw_socket()),
            GetCurrentProcessId(),
            &raw mut info,
        )
    })?;
    let socket = wsa_socket(
        info.iAddressFamily,
        info.iSocketType,
        info.iProtocol,
        Some(&info),
    )?;
    clear_inherit(&socket)?;
    Ok(socket)
}
