//! Name resolution with `GetAddrInfoW`.

use core::{ptr, slice};

use alloc_crate::vec::Vec;
use windows_sys::Win32::Networking::WinSock::{
    ADDRINFOW, FreeAddrInfoW, GetAddrInfoW, SOCK_STREAM, SOCKADDR_STORAGE,
};

use super::init;
use crate::{
    io,
    net::{SocketAddr, sockaddr},
};

/// The error for a host name with a NUL character, as std reports it.
const NUL_IN_HOST: io::Error = io::const_error!(
    io::ErrorKind::InvalidInput,
    "file name contained an unexpected NUL byte",
);

/// The IPv4 and IPv6 addresses `GetAddrInfoW` found for a host, in its
/// order, each with the port that was asked for.
pub(crate) struct LookupHost {
    /// The list `GetAddrInfoW` returned, freed when this is dropped.
    head: *mut ADDRINFOW,
    /// The next entry to look at: null, or an entry of the list.
    next: *const ADDRINFOW,
    port: u16,
}

/// Resolves `host` for stream sockets, as std does with `getaddrinfo`,
/// passing the name as UTF-16 so that it resolves whatever the code page.
///
/// # Errors
///
/// Fails if `host` contains a NUL character, or with the resolver's error.
pub(crate) fn lookup_host(host: &str, port: u16) -> io::Result<LookupHost> {
    init()?;
    if host.as_bytes().contains(&0) {
        return Err(NUL_IN_HOST);
    }
    let mut wide = Vec::with_capacity(host.len() + 1);
    wide.extend(host.encode_utf16());
    wide.push(0);
    let hints = ADDRINFOW {
        ai_socktype: SOCK_STREAM,
        ..ADDRINFOW::default()
    };
    let mut head = ptr::null_mut();
    // SAFETY: `wide` is NUL-terminated, `hints` is a valid `ADDRINFOW` with
    // null pointers, `head` is valid for writes, and the service is null.
    let code = unsafe {
        GetAddrInfoW(
            wide.as_ptr(),
            ptr::null(),
            &raw const hints,
            &raw mut head,
        )
    };
    // The resolver returns its error, which is also the last error.
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code));
    }
    Ok(LookupHost {
        head,
        next: head,
        port,
    })
}

impl Iterator for LookupHost {
    type Item = SocketAddr;

    fn next(&mut self) -> Option<SocketAddr> {
        // Each pass moves one entry along the list, which the resolver
        // ended with a null link, so the loop ends.
        loop {
            // SAFETY: `next` is null or an entry of the list, which stays
            // allocated until `self` is dropped.
            let entry = unsafe { self.next.as_ref() }?;
            self.next = entry.ai_next;
            if let Some(mut addr) = entry_addr(entry) {
                addr.set_port(self.port);
                return Some(addr);
            }
        }
    }
}

/// The IP address of an entry of the list; entries of other families are
/// skipped, as in std.
fn entry_addr(entry: &ADDRINFOW) -> Option<SocketAddr> {
    if entry.ai_addr.is_null() {
        return None;
    }
    // No socket address is longer; an IP address needs 28 bytes at most.
    let len = entry.ai_addrlen.min(size_of::<SOCKADDR_STORAGE>());
    // SAFETY: the resolver points a non-null `ai_addr` at an address of
    // `ai_addrlen` bytes, allocated with the list, which outlives `entry`;
    // `len` is no more than that, and bytes have no alignment.
    let bytes =
        unsafe { slice::from_raw_parts(entry.ai_addr.cast::<u8>(), len) };
    sockaddr::from_bytes(bytes)
}

impl Drop for LookupHost {
    fn drop(&mut self) {
        // A successful resolution returns at least one entry; the check
        // keeps an empty list from reaching `FreeAddrInfoW` all the same.
        if !self.head.is_null() {
            // SAFETY: `head` is the list `GetAddrInfoW` returned, freed once.
            unsafe { FreeAddrInfoW(self.head) };
        }
    }
}
