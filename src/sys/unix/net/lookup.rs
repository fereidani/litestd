//! Name resolution with `getaddrinfo`.

use core::{
    ffi::{CStr, c_int},
    ptr, slice,
};

use alloc_crate::{borrow::Cow, format};

use crate::{
    io,
    net::{
        SocketAddr,
        sockaddr::{self, Storage},
    },
    sys::os::{self, STACK_BUF},
};

/// The IPv4 and IPv6 addresses `getaddrinfo` found for a host, in its
/// order, each with the port that was asked for.
pub(crate) struct LookupHost {
    /// The list `getaddrinfo` returned, freed when this is dropped.
    head: *mut libc::addrinfo,
    /// The next entry to look at: null, or an entry of the list.
    next: *const libc::addrinfo,
    port: u16,
}

/// Resolves `host` for stream sockets, as std does. Fails if `host`
/// contains a NUL byte, or with the resolver's error.
pub(crate) fn lookup_host(host: &str, port: u16) -> io::Result<LookupHost> {
    let mut buf = STACK_BUF;
    let host = os::name_cstr(host.as_bytes(), &mut buf)?;
    let hints = libc::addrinfo {
        ai_flags: 0,
        ai_family: libc::AF_UNSPEC,
        ai_socktype: libc::SOCK_STREAM,
        ai_protocol: 0,
        ai_addrlen: 0,
        ai_addr: ptr::null_mut(),
        ai_canonname: ptr::null_mut(),
        ai_next: ptr::null_mut(),
    };
    let mut head = ptr::null_mut();
    // `getaddrinfo` reads the environment: hold `env`'s read lock against a
    // concurrent `set_var`, as std does.
    #[cfg(feature = "env")]
    let _env = crate::sys::env::EnvGuard::read();
    // SAFETY: `host` is a C string, `hints` a valid `addrinfo` with null
    // pointers, and `head` valid for writes; there is no service name.
    let code = unsafe {
        libc::getaddrinfo(
            host.as_ptr(),
            ptr::null(),
            &raw const hints,
            &raw mut head,
        )
    };
    if code != 0 {
        return Err(gai_error(code));
    }
    Ok(LookupHost {
        head,
        next: head,
        port,
    })
}

/// Converts a `getaddrinfo` failure into the error std reports: the OS
/// error for `EAI_SYSTEM`, and the resolver's message otherwise.
#[cold]
fn gai_error(code: c_int) -> io::Error {
    // Read first: the workaround below may change `errno`.
    let system = (code == libc::EAI_SYSTEM).then(io::Error::last_os_error);
    on_resolver_failure();
    if let Some(e) = system {
        return e;
    }
    // SAFETY: `gai_strerror` has no preconditions.
    let detail = unsafe { libc::gai_strerror(code) };
    let detail = if detail.is_null() {
        Cow::Borrowed("")
    } else {
        // SAFETY: a non-null result is a C string that the C library keeps
        // for the life of the process.
        unsafe { CStr::from_ptr(detail) }.to_string_lossy()
    };
    io::Error::new(
        io::ErrorKind::Uncategorized,
        format!("failed to lookup address information: {detail}"),
    )
}

/// Makes glibc before 2.26 read `/etc/resolv.conf` again, as std does:
/// those versions keep what they first read, so a long-running program
/// would miss network changes. Other C libraries do not need the call.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn on_resolver_failure() {
    let version = os::glibc_version();
    if version.is_some_and(|version| version < (2, 26)) {
        // SAFETY: glibc's `res_init` has no preconditions and is thread-safe.
        unsafe { libc::res_init() };
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
const fn on_resolver_failure() {}

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
fn entry_addr(entry: &libc::addrinfo) -> Option<SocketAddr> {
    if entry.ai_addr.is_null() {
        return None;
    }
    // Capping at the largest socket address changes no IP entry and keeps
    // the slice within `isize::MAX` bytes.
    let len = (entry.ai_addrlen as usize).min(Storage::LEN.into());
    // SAFETY: the resolver points a non-null `ai_addr` at an address of
    // `ai_addrlen` bytes, at least `len`, allocated with the list, which
    // outlives `entry`.
    let bytes =
        unsafe { slice::from_raw_parts(entry.ai_addr.cast::<u8>(), len) };
    sockaddr::from_bytes(bytes)
}

impl Drop for LookupHost {
    fn drop(&mut self) {
        // A successful resolution returns at least one entry; the check
        // keeps an empty list from reaching `freeaddrinfo` all the same.
        if !self.head.is_null() {
            // SAFETY: `head` is the list `getaddrinfo` returned, freed here
            // once; nothing uses it afterwards.
            unsafe { libc::freeaddrinfo(self.head) };
        }
    }
}
