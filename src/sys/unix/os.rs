//! OS error codes, the handling of C library results, descriptors and C
//! strings, and process control. Each item compiles only with the features
//! of its callers.

#[cfg(any(feature = "env", feature = "fs"))]
pub(crate) mod path;

#[cfg(any(
    feature = "env",
    feature = "fs",
    all(unix, feature = "net"),
    feature = "process"
))]
use core::ptr;
#[cfg(feature = "process")]
use core::sync::atomic::{AtomicPtr, Ordering};
#[cfg(any(feature = "env", feature = "fs", all(unix, feature = "net")))]
use core::{ffi::CStr, slice};
#[cfg(feature = "io")]
use core::{ffi::c_int, fmt, mem::MaybeUninit};

#[cfg(any(feature = "env", feature = "fs", all(unix, feature = "net")))]
use alloc_crate::{borrow::Cow, ffi::CString};

#[cfg(feature = "io")]
use crate::{
    io::{self, ErrorKind, IoSlice, IoSliceMut},
    os::fd::RawFd,
};

/// The unwinder, which a `no_std` binary needs although it never unwinds:
/// the precompiled `core` and `alloc` are built for unwinding and call it
/// from cleanup code that only LTO can remove, and not always. WebAssembly
/// builds them without unwinding.
#[cfg(all(unix, not(any(test, feature = "test-with-std"))))]
#[allow(
    clippy::duplicated_attributes,
    reason = "each static library needs a `link` of its own"
)]
mod unwinder {
    use core::{ffi::c_void, hint};

    // Links the unwinder std links. The block comes before the C library's
    // below because a static unwinder needs the C library.
    #[cfg_attr(
        all(
            target_feature = "crt-static",
            any(
                target_os = "android",
                all(
                    target_env = "musl",
                    not(any(target_arch = "x86_64", target_arch = "aarch64"))
                )
            )
        ),
        link(name = "unwind", kind = "static", modifiers = "-bundle")
    )]
    #[cfg_attr(
        all(target_env = "gnu", target_feature = "crt-static"),
        link(name = "gcc_eh", kind = "static", modifiers = "-bundle")
    )]
    #[cfg_attr(
        all(target_os = "android", not(target_feature = "crt-static")),
        link(name = "unwind")
    )]
    #[cfg_attr(
        all(target_os = "linux", not(target_feature = "crt-static")),
        link(name = "gcc_s")
    )]
    // Apple's unwinder is part of libSystem.
    #[cfg_attr(target_vendor = "apple", link(name = "System"))]
    #[cfg_attr(
        any(
            target_os = "netbsd",
            all(target_os = "freebsd", not(target_feature = "crt-static"))
        ),
        link(name = "gcc_s")
    )]
    #[cfg_attr(
        all(target_os = "freebsd", target_feature = "crt-static"),
        link(name = "gcc", kind = "static", modifiers = "-bundle")
    )]
    #[cfg_attr(
        all(target_os = "freebsd", target_feature = "crt-static"),
        link(name = "gcc_eh", kind = "static", modifiers = "-bundle")
    )]
    #[cfg_attr(target_os = "openbsd", link(name = "c++abi"))]
    #[cfg_attr(target_os = "dragonfly", link(name = "gcc_pic"))]
    unsafe extern "C" {
        fn _Unwind_Resume(exception: *mut c_void) -> !;
    }

    // glibc's `libc.a` refers back to the unwinder (the cleanup code of
    // `pthread_once` names `__gcc_personality_v0`), and GNU ld scans each
    // archive once, in order: so `libc.a` sits between two `libgcc_eh.a`
    // copies, each resolving what the one before it needs.
    #[cfg(all(target_env = "gnu", target_feature = "crt-static"))]
    #[link(name = "c", kind = "static", modifiers = "-bundle")]
    #[link(name = "gcc_eh", kind = "static", modifiers = "-bundle")]
    unsafe extern "C" {}

    // A static musl binary gets a weak `_Unwind_Resume` that aborts, instead
    // of the 80 KiB unwinder: without an unwinder no unwinding starts, so it
    // is never called. An unwinder linked by other code replaces it, and a
    // static binary allows no later interposition.
    #[cfg(all(
        target_env = "musl",
        target_feature = "crt-static",
        target_arch = "x86_64"
    ))]
    core::arch::global_asm!(
        ".pushsection .text._Unwind_Resume,\"ax\",@progbits",
        ".weak _Unwind_Resume",
        ".type _Unwind_Resume,@function",
        "_Unwind_Resume:",
        "jmp abort",
        ".size _Unwind_Resume, . - _Unwind_Resume",
        ".popsection",
    );
    #[cfg(all(
        target_env = "musl",
        target_feature = "crt-static",
        target_arch = "aarch64"
    ))]
    core::arch::global_asm!(
        ".pushsection .text._Unwind_Resume,\"ax\",%progbits",
        ".weak _Unwind_Resume",
        ".type _Unwind_Resume,%function",
        "_Unwind_Resume:",
        "b abort",
        ".size _Unwind_Resume, . - _Unwind_Resume",
        ".popsection",
    );

    /// Makes the linker resolve `_Unwind_Resume` before it reaches `core`
    /// and `alloc`: GNU ld only resolves references it has already seen,
    /// and rustc places the unwinder before `alloc`. The personality routine,
    /// which the linker loads first, calls this, so the reference is linked
    /// exactly when the cleanup code that needs it is.
    pub(crate) fn reference() {
        // Only the address is taken; nothing calls the function.
        hint::black_box(
            _Unwind_Resume as unsafe extern "C" fn(*mut c_void) -> !,
        );
    }
}

#[cfg(all(unix, not(any(test, feature = "test-with-std"))))]
pub(crate) use unwinder::reference as reference_unwinder;

// On musl and WASI the `libc` crate links the C library only for std, so a
// `no_std` binary links it here, after the unwinder, which needs it.
// Linking it twice alongside std is harmless.
#[cfg(any(target_env = "musl", target_os = "wasi"))]
#[cfg_attr(
    target_feature = "crt-static",
    link(name = "c", kind = "static", modifiers = "-bundle")
)]
#[cfg_attr(not(target_feature = "crt-static"), link(name = "c"))]
unsafe extern "C" {}

/// Returns the address of the calling thread's `errno`, which is unique
/// among live threads and so doubles as a cheap thread identity.
#[cfg(any(
    feature = "io",
    all(unix, feature = "sync"),
    feature = "thread",
    feature = "stdio",
    feature = "process",
    all(
        feature = "panic-location",
        not(any(
            test,
            feature = "test-with-std",
            feature = "custom-panic-handler"
        ))
    )
))]
pub(crate) fn errno_location() -> *mut libc::c_int {
    // SAFETY: the errno accessor has no preconditions.
    unsafe {
        #[cfg(any(
            target_os = "linux",
            target_os = "dragonfly",
            target_os = "wasi"
        ))]
        let errno = libc::__errno_location();
        #[cfg(any(
            target_os = "android",
            target_os = "netbsd",
            target_os = "openbsd"
        ))]
        let errno = libc::__errno();
        #[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
        let errno = libc::__error();
        errno
    }
}

/// Returns the calling thread's `errno`.
#[cfg(any(
    feature = "io",
    all(unix, feature = "sync"),
    feature = "thread",
    feature = "stdio",
    all(
        feature = "panic-location",
        not(any(
            test,
            feature = "test-with-std",
            feature = "custom-panic-handler"
        ))
    )
))]
pub(crate) fn errno() -> i32 {
    // SAFETY: `errno_location` points to the calling thread's live `errno`.
    unsafe { *errno_location() }
}

/// Whether the error code means that a signal interrupted the call: the
/// `ErrorKind::Interrupted` of `decode_error_kind`, without its table.
#[cfg(feature = "io")]
#[inline]
pub(crate) const fn is_interrupted(code: i32) -> bool {
    code == libc::EINTR
}

/// Converts a C library result of -1 to the error in `errno`.
#[cfg(any(feature = "env", feature = "fs", all(unix, feature = "io")))]
pub(crate) fn cvt<T: Copy + PartialEq + From<i8>>(r: T) -> io::Result<T> {
    if r == T::from(-1) {
        Err(io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

/// Like [`cvt`], but repeats the call while a signal interrupts it.
#[cfg(any(
    feature = "fs",
    all(unix, any(feature = "command", feature = "net"))
))]
pub(crate) fn cvt_r<T: Copy + PartialEq + From<i8>>(
    mut f: impl FnMut() -> T,
) -> io::Result<T> {
    // `EINTR` means the call took no effect, so repeating it is safe; the
    // loop ends at the first result that is not an interruption.
    loop {
        match cvt(f()) {
            Err(e) if e.is_interrupted() => {}
            r => return r,
        }
    }
}

/// Converts a byte count, where -1, the only negative result, means failure.
#[cfg(feature = "io")]
pub(crate) fn cvt_len(r: isize) -> io::Result<usize> {
    usize::try_from(r).map_err(|_| io::Error::last_os_error())
}

/// The most bytes one `read` or `write` asks for, `ssize_t::MAX`: POSIX
/// leaves larger counts to the implementation. macOS rejects counts above
/// `INT_MAX` with `EINVAL`, so std caps them there.
#[cfg(any(
    feature = "io",
    feature = "stdio",
    all(
        feature = "panic-location",
        not(any(
            test,
            feature = "test-with-std",
            feature = "custom-panic-handler"
        ))
    )
))]
pub(crate) const MAX_LEN: usize = if cfg!(target_vendor = "apple") {
    libc::c_int::MAX.unsigned_abs() as usize
} else {
    isize::MAX.unsigned_abs()
};

/// Returns how many of `len` buffers one vectored call may take: at most
/// 1024, Linux's `UIO_MAXIOV` and macOS's `IOV_MAX`. The rest wait for the
/// next call, as after any partial transfer.
#[cfg(feature = "io")]
pub(crate) fn iov_count(len: usize) -> c_int {
    const IOV_MAX: c_int = 1024;
    c_int::try_from(len).map_or(IOV_MAX, |len| len.min(IOV_MAX))
}

/// Reads from `fd` into `buf`, which only the kernel writes: it initializes
/// the bytes it reports read and touches no others.
#[cfg(feature = "io")]
pub(crate) fn read(
    fd: RawFd,
    buf: &mut [MaybeUninit<u8>],
) -> io::Result<usize> {
    let len = buf.len().min(MAX_LEN);
    // SAFETY: `buf` is valid for writes of `len` bytes.
    cvt_len(unsafe { libc::read(fd, buf.as_mut_ptr().cast(), len) })
}

/// Reads from `fd` into `bufs`, in order.
#[cfg(feature = "io")]
pub(crate) fn read_vectored(
    fd: RawFd,
    bufs: &mut [IoSliceMut<'_>],
) -> io::Result<usize> {
    let count = iov_count(bufs.len());
    // SAFETY: `IoSliceMut` has the layout of `iovec`, each describing memory
    // valid for writes, and `count` is at most `bufs.len()`.
    cvt_len(unsafe { libc::readv(fd, bufs.as_ptr().cast(), count) })
}

/// Writes `buf` to `fd`.
#[cfg(feature = "io")]
pub(crate) fn write(fd: RawFd, buf: &[u8]) -> io::Result<usize> {
    let len = buf.len().min(MAX_LEN);
    // SAFETY: `buf` is valid for reads of `len` bytes.
    cvt_len(unsafe { libc::write(fd, buf.as_ptr().cast(), len) })
}

/// Writes `bufs` to `fd`, in order.
#[cfg(feature = "io")]
pub(crate) fn write_vectored(
    fd: RawFd,
    bufs: &[IoSlice<'_>],
) -> io::Result<usize> {
    let count = iov_count(bufs.len());
    // SAFETY: `IoSlice` has the layout of `iovec`, each describing memory
    // valid for reads, and `count` is at most `bufs.len()`.
    cvt_len(unsafe { libc::writev(fd, bufs.as_ptr().cast(), count) })
}

/// Closes a descriptor that its owner gives up. Errors are ignored, as in
/// std: Linux frees the descriptor even when `close` fails, so a retry could
/// close one that another thread has just opened.
#[cfg(feature = "io")]
pub(crate) fn close_fd(fd: RawFd) {
    // SAFETY: the caller owns `fd` and never uses it again.
    let r = unsafe { libc::close(fd) };
    debug_assert!(
        r == 0 || errno() != libc::EBADF,
        "I/O safety violation: owned file descriptor already closed",
    );
}

/// Duplicates `fd` into a new close-on-exec descriptor numbered 3 or above,
/// which never takes the place of a standard stream.
#[cfg(all(unix, feature = "io"))]
pub(crate) fn duplicate_fd(fd: RawFd) -> io::Result<RawFd> {
    // SAFETY: `F_DUPFD_CLOEXEC` touches no memory.
    cvt(unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) })
}

/// Fails as in std: WASI cannot duplicate a descriptor.
#[cfg(all(target_os = "wasi", feature = "io"))]
#[allow(clippy::missing_const_for_fn, reason = "stands in for an OS call")]
pub(crate) fn duplicate_fd(_fd: RawFd) -> io::Result<RawFd> {
    Err(crate::sys::unsupported::UNSUPPORTED)
}

/// Sets close-on-exec on a new descriptor, which macOS cannot create with
/// the flag, with the `ioctl` std uses there.
#[cfg(all(target_vendor = "apple", feature = "io"))]
pub(crate) fn set_cloexec(fd: RawFd) -> io::Result<()> {
    // SAFETY: `FIOCLEX` touches no memory.
    cvt(unsafe { libc::ioctl(fd, libc::FIOCLEX) }).map(drop)
}

/// Returns the environment: the C library's aligned array of `KEY=VALUE`
/// C strings up to a null pointer, itself possibly null.
///
/// # Safety
///
/// No thread may change the environment meanwhile: litestd's writers hold
/// `env`'s lock alone, and `env::set_var`'s contract rules out the others.
#[cfg(any(feature = "env", all(unix, feature = "command")))]
pub(crate) unsafe fn environ() -> *const *const core::ffi::c_char {
    #[cfg(not(target_vendor = "apple"))]
    {
        let slot = environ_slot();
        if slot.is_null() {
            return ptr::null();
        }
        // SAFETY: `slot` is the C library's `environ`, and the caller rules
        // out writers.
        unsafe { slot.read() }
    }
    // macOS documents `_NSGetEnviron` as the way to reach it; std uses it.
    // SAFETY: it returns the address of the pointer, which the caller rules
    // out writers of.
    #[cfg(target_vendor = "apple")]
    unsafe {
        let environ = libc::_NSGetEnviron().read();
        environ.cast::<*const core::ffi::c_char>().cast_const()
    }
}

/// The `KEY=VALUE` entries of [`environ`], without their NULs, borrowed
/// from the lock that `'a` holds.
#[cfg(any(feature = "env", all(unix, feature = "command")))]
#[derive(Clone)]
pub(crate) struct Entries<'a> {
    /// The next slot of `environ`, or null if `environ` is null.
    next: *const *const core::ffi::c_char,
    _lock: core::marker::PhantomData<&'a ()>,
}

#[cfg(any(feature = "env", all(unix, feature = "command")))]
impl<'a> Entries<'a> {
    /// The entries of `environ`, which stay unchanged while `_lock` lives.
    ///
    /// # Safety
    ///
    /// As for [`environ`], while `_lock` lives.
    pub(crate) unsafe fn new<L>(_lock: &'a L) -> Self {
        Self {
            // SAFETY: the caller rules out writers.
            next: unsafe { environ() },
            _lock: core::marker::PhantomData,
        }
    }
}

#[cfg(any(feature = "env", all(unix, feature = "command")))]
impl<'a> Iterator for Entries<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        if self.next.is_null() {
            return None;
        }
        // SAFETY: `next` is a slot of the aligned, null-terminated array
        // `environ`, unchanged while the lock lives, and the walk stops at
        // the null.
        let entry = unsafe { self.next.read() };
        if entry.is_null() {
            return None;
        }
        // SAFETY: a non-null slot is followed by another slot.
        self.next = unsafe { self.next.add(1) };
        // SAFETY: entries are C strings, unchanged while the lock lives.
        Some(unsafe { core::ffi::CStr::from_ptr(entry) }.to_bytes())
    }
}

/// The address of the C library's `environ` variable.
#[cfg(all(
    any(feature = "env", all(unix, feature = "command")),
    not(any(
        target_vendor = "apple",
        all(target_os = "freebsd", not(target_feature = "crt-static"))
    ))
))]
pub(crate) fn environ_slot() -> *mut *const *const core::ffi::c_char {
    unsafe extern "C" {
        /// The process's environment block, which the C library reads.
        #[link_name = "environ"]
        static mut ENVIRON: *const *const core::ffi::c_char;
    }
    &raw mut ENVIRON
}

/// The address of the C library's `environ` variable, or null if it cannot
/// be found. On FreeBSD it lives in the program's startup file, not in
/// libc, so a shared library cannot link it: a dynamic program finds it
/// once with `dlsym`, as std's weak reference does. A static one links it,
/// as `dlsym` has no symbol table to search there.
#[cfg(all(
    any(feature = "env", all(unix, feature = "command")),
    target_os = "freebsd",
    not(target_feature = "crt-static")
))]
pub(crate) fn environ_slot() -> *mut *const *const core::ffi::c_char {
    use core::{
        ffi::c_void,
        sync::atomic::{AtomicPtr, Ordering},
    };
    /// Marks the lookup as not done yet; `dlsym` never returns it.
    const UNKNOWN: *mut c_void = ptr::dangling_mut();
    // Relaxed suffices: the address never changes, and racing threads find
    // the same one.
    static FOUND: AtomicPtr<c_void> = AtomicPtr::new(UNKNOWN);
    let mut found = FOUND.load(Ordering::Relaxed);
    if found == UNKNOWN {
        // SAFETY: the name is a C string; `RTLD_DEFAULT` searches the
        // loaded objects.
        found = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"environ".as_ptr()) };
        FOUND.store(found, Ordering::Relaxed);
    }
    found.cast()
}

/// Strings shorter than this many bytes become C strings on the stack. The
/// limit keeps frames well under a page, so they need no stack probe, and
/// holds every DNS name.
#[cfg(any(feature = "env", feature = "fs", all(unix, feature = "net")))]
pub(crate) const MAX_STACK: usize = 384;

/// Stack space for one C string, uninitialized until [`cstr`] fills it.
#[cfg(any(feature = "env", feature = "fs", all(unix, feature = "net")))]
pub(crate) type StackBuf = [MaybeUninit<u8>; MAX_STACK];

/// A new [`StackBuf`]: copying it initializes nothing.
#[cfg(any(feature = "env", feature = "fs", all(unix, feature = "net")))]
pub(crate) const STACK_BUF: StackBuf = [MaybeUninit::uninit(); MAX_STACK];

/// std's error for a path or host name that contains a NUL byte.
#[cfg(any(feature = "env", feature = "fs", all(unix, feature = "net")))]
const NUL_IN_NAME: io::Error = io::const_error!(
    io::ErrorKind::InvalidInput,
    "file name contained an unexpected NUL byte",
);

/// std's error for a program, argument, variable or directory of a command
/// that contains a NUL byte.
#[cfg(all(unix, feature = "command"))]
pub(crate) const NUL_IN_DATA: io::Error = io::const_error!(
    io::ErrorKind::InvalidInput,
    "nul byte found in provided data",
);

/// Converts `bytes` to a C string in `buf`, or on the heap if it does not
/// fit. Returns `None` if `bytes` contains a NUL, which would cut it short.
#[cfg(any(feature = "env", feature = "fs", all(unix, feature = "net")))]
pub(crate) fn cstr<'a>(
    bytes: &[u8],
    buf: &'a mut StackBuf,
) -> Option<Cow<'a, CStr>> {
    let len = bytes.len();
    if len >= MAX_STACK {
        return heap_cstr(bytes);
    }
    let dst = buf.as_mut_ptr().cast::<u8>();
    // SAFETY: `len` is below `MAX_STACK`, the size of `buf`, so the copy and
    // the NUL after it stay inside `buf`, which `bytes` cannot overlap while
    // `buf` is borrowed mutably. The view covers the `len + 1` bytes just
    // written and borrows `buf` for `'a`. A checked copy would cost each
    // caller a few bytes and branches.
    let with_nul = unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), dst, len);
        dst.add(len).write(0);
        slice::from_raw_parts(dst, len + 1)
    };
    CStr::from_bytes_with_nul(with_nul).ok().map(Cow::Borrowed)
}

#[cfg(any(feature = "env", feature = "fs", all(unix, feature = "net")))]
#[cold]
#[inline(never)]
fn heap_cstr(bytes: &[u8]) -> Option<Cow<'static, CStr>> {
    CString::new(bytes).ok().map(Cow::Owned)
}

/// Converts a path or host name as [`cstr`] does. A NUL byte, which would
/// truncate it, fails with `InvalidInput`, as in std.
#[cfg(any(feature = "env", feature = "fs", all(unix, feature = "net")))]
pub(crate) fn name_cstr<'a>(
    bytes: &[u8],
    buf: &'a mut StackBuf,
) -> io::Result<Cow<'a, CStr>> {
    cstr(bytes, buf).ok_or(NUL_IN_NAME)
}

/// The major and minor version of the glibc the program runs with, which
/// name resolution and `posix_spawn` work around when it is old.
#[cfg(all(
    target_os = "linux",
    target_env = "gnu",
    any(feature = "command", feature = "net")
))]
pub(crate) fn glibc_version() -> Option<(u32, u32)> {
    // SAFETY: `gnu_get_libc_version` has no preconditions.
    let version = unsafe { libc::gnu_get_libc_version() };
    if version.is_null() {
        return None;
    }
    // SAFETY: a non-null result is a C string that glibc keeps for the life
    // of the process.
    let version = unsafe { core::ffi::CStr::from_ptr(version) };
    let mut parts = version.to_str().ok()?.split('.').map(str::parse::<u32>);
    match (parts.next(), parts.next()) {
        (Some(Ok(major)), Some(Ok(minor))) => Some((major, minor)),
        _ => None,
    }
}

/// Classifies an `errno` value the way std does.
#[cfg(feature = "io")]
pub(crate) fn decode_error_kind(code: i32) -> ErrorKind {
    usize::try_from(code)
        .ok()
        .and_then(|code| KINDS.get(code))
        .copied()
        .unwrap_or(ErrorKind::Uncategorized)
}

/// The codes that std classifies, with their kinds. Some are the same value
/// on some targets, as `EAGAIN` and `EWOULDBLOCK` are.
#[cfg(feature = "io")]
const CLASSIFIED: [(c_int, ErrorKind); 43] = [
    (libc::E2BIG, ErrorKind::ArgumentListTooLong),
    (libc::EACCES, ErrorKind::PermissionDenied),
    (libc::EADDRINUSE, ErrorKind::AddrInUse),
    (libc::EADDRNOTAVAIL, ErrorKind::AddrNotAvailable),
    (libc::EAGAIN, ErrorKind::WouldBlock),
    (libc::EBUSY, ErrorKind::ResourceBusy),
    (libc::ECONNABORTED, ErrorKind::ConnectionAborted),
    (libc::ECONNREFUSED, ErrorKind::ConnectionRefused),
    (libc::ECONNRESET, ErrorKind::ConnectionReset),
    (libc::EDEADLK, ErrorKind::Deadlock),
    (libc::EDQUOT, ErrorKind::QuotaExceeded),
    (libc::EEXIST, ErrorKind::AlreadyExists),
    (libc::EFBIG, ErrorKind::FileTooLarge),
    (libc::EHOSTUNREACH, ErrorKind::HostUnreachable),
    (libc::EINPROGRESS, ErrorKind::InProgress),
    (libc::EINTR, ErrorKind::Interrupted),
    (libc::EINVAL, ErrorKind::InvalidInput),
    (libc::EISDIR, ErrorKind::IsADirectory),
    (libc::ELOOP, ErrorKind::FilesystemLoop),
    (libc::EMFILE, ErrorKind::TooManyOpenFiles),
    (libc::EMLINK, ErrorKind::TooManyLinks),
    (libc::ENAMETOOLONG, ErrorKind::InvalidFilename),
    (libc::ENETDOWN, ErrorKind::NetworkDown),
    (libc::ENETUNREACH, ErrorKind::NetworkUnreachable),
    (libc::ENFILE, ErrorKind::TooManyOpenFiles),
    (libc::ENOENT, ErrorKind::NotFound),
    (libc::ENOMEM, ErrorKind::OutOfMemory),
    (libc::ENOSPC, ErrorKind::StorageFull),
    (libc::ENOSYS, ErrorKind::Unsupported),
    (libc::ENOTCONN, ErrorKind::NotConnected),
    (libc::ENOTDIR, ErrorKind::NotADirectory),
    (libc::ENOTEMPTY, ErrorKind::DirectoryNotEmpty),
    (libc::ENOTSUP, ErrorKind::Unsupported),
    (libc::EOPNOTSUPP, ErrorKind::Unsupported),
    (libc::EPERM, ErrorKind::PermissionDenied),
    (libc::EPIPE, ErrorKind::BrokenPipe),
    (libc::EROFS, ErrorKind::ReadOnlyFilesystem),
    (libc::ESPIPE, ErrorKind::NotSeekable),
    (libc::ESTALE, ErrorKind::StaleNetworkFileHandle),
    (libc::ETIMEDOUT, ErrorKind::TimedOut),
    (libc::ETXTBSY, ErrorKind::ExecutableFileBusy),
    (libc::EWOULDBLOCK, ErrorKind::WouldBlock),
    (libc::EXDEV, ErrorKind::CrossesDevices),
];

/// [`CLASSIFIED`] indexed by code, one byte each. A `match` compiles to a
/// jump table with a stub for each case, several times the size: its cases,
/// spread over 122 values on Linux, are too sparse for LLVM to make a table
/// of their kinds.
#[cfg(feature = "io")]
static KINDS: [ErrorKind; kinds_len()] = {
    let mut kinds = [ErrorKind::Uncategorized; kinds_len()];
    let mut i = 0;
    while i < CLASSIFIED.len() {
        let (code, kind) = CLASSIFIED[i];
        kinds[code.unsigned_abs() as usize] = kind;
        i += 1;
    }
    kinds
};

/// One more than the largest code in [`CLASSIFIED`], all positive.
#[cfg(feature = "io")]
const fn kinds_len() -> usize {
    let mut len = 0;
    let mut i = 0;
    while i < CLASSIFIED.len() {
        let code = CLASSIFIED[i].0;
        assert!(code > 0, "error codes are positive");
        if code.unsigned_abs() as usize >= len {
            len = code.unsigned_abs() as usize + 1;
        }
        i += 1;
    }
    len
}

/// Writes the C library's description of `code`, as `strerror` gives it.
/// `Display` and `Debug` both call it, so it stays out of line.
#[cfg(feature = "io")]
#[inline(never)]
pub(crate) fn fmt_error_message(
    code: i32,
    f: &mut fmt::Formatter<'_>,
) -> fmt::Result {
    // Every glibc, musl and macOS message fits; a longer one is truncated.
    let mut buf = [0u8; 128];
    // SAFETY: `buf` is valid for writes of all but its last byte, which
    // `strerror_r` is not offered and which stays NUL.
    let r = unsafe {
        libc::strerror_r(code, buf.as_mut_ptr().cast(), buf.len() - 1)
    };
    // The call fails only for an unknown code or a short buffer, and still
    // leaves the best text it has.
    debug_assert!(r == 0 || r == libc::EINVAL || r == libc::ERANGE);
    // SAFETY: the last byte of `buf` is NUL, so `strlen` stops inside it.
    let len = unsafe { libc::strlen(buf.as_ptr().cast()) };
    // Messages are ASCII in the C locale, the only one a program that never
    // calls `setlocale` uses. Anything else is shown lossily, as in std.
    let mut lossy = [0u8; 3 * 128];
    f.write_str(to_lossy(buf.get(..len).unwrap_or_default(), &mut lossy))
}

/// Copies `bytes` into `buf` with each invalid UTF-8 sequence replaced by
/// U+FFFD, which takes three bytes where the sequence took one or more.
#[cfg(feature = "io")]
fn to_lossy<'a>(bytes: &[u8], buf: &'a mut [u8; 3 * 128]) -> &'a str {
    let mut len = 0;
    for chunk in bytes.utf8_chunks() {
        let invalid = if chunk.invalid().is_empty() {
            ""
        } else {
            "\u{FFFD}"
        };
        for piece in [chunk.valid(), invalid] {
            let end = len + piece.len();
            if let Some(dst) = buf.get_mut(len..end) {
                dst.copy_from_slice(piece.as_bytes());
                len = end;
            }
        }
    }
    // The copy is whole `str`s one after another: one valid chunk.
    let copy = buf.get(..len).unwrap_or_default();
    copy.utf8_chunks().next().map_or("", |chunk| chunk.valid())
}

/// Terminates the process abnormally, without running destructors or exit
/// handlers.
// Test builds link std and compile out the panic handler and the
// personality routine; with no module feature on, nothing else calls this.
#[cfg_attr(any(test, feature = "test-with-std"), allow(dead_code))]
pub(crate) fn abort() -> ! {
    // SAFETY: `abort` has no preconditions and does not return.
    unsafe { libc::abort() }
}

/// Terminates the process with `code`, running the C library's exit
/// handlers.
#[cfg(feature = "process")]
pub(crate) fn exit(code: i32) -> ! {
    /// The `errno` address of the first thread to call `exit`, or null.
    static EXITING: AtomicPtr<libc::c_int> = AtomicPtr::new(ptr::null_mut());

    // C forbids concurrent calls to `exit`, and older glibc versions corrupt
    // their exit handler list when that happens. Only the first caller goes
    // on. The exchange publishes no data, so `Relaxed` suffices.
    let this_thread = errno_location();
    match EXITING.compare_exchange(
        ptr::null_mut(),
        this_thread,
        Ordering::Relaxed,
        Ordering::Relaxed,
    ) {
        // SAFETY: this is the only call to `exit` that litestd makes, and
        // the exchange above lets exactly one thread reach it.
        Ok(_) => unsafe { libc::exit(code) },
        // An exit handler called `exit` again, which C leaves undefined.
        Err(first) if first == this_thread => abort(),
        // Another thread is running the exit handlers; the process ends
        // when it finishes. `pause` returns only after a signal handler
        // ran, so this waits for the other thread without spinning.
        #[cfg(unix)]
        Err(_) => loop {
            // SAFETY: `pause` has no preconditions.
            unsafe { libc::pause() };
        },
        // WASI runs one thread, which is the first caller.
        #[cfg(not(unix))]
        Err(_) => abort(),
    }
}

/// Panics as std does: WASI has no process ids.
#[cfg(all(target_os = "wasi", feature = "process"))]
#[cold]
#[track_caller]
#[allow(clippy::panic, reason = "std panics here")]
pub(crate) fn id() -> u32 {
    panic!("no pids on this platform")
}

/// Returns the id of the calling process.
#[cfg(all(unix, feature = "process"))]
pub(crate) fn id() -> u32 {
    // SAFETY: `getpid` has no preconditions and always succeeds.
    let pid = unsafe { libc::getpid() };
    // Process ids are positive, so the conversion never falls back.
    u32::try_from(pid).unwrap_or(0)
}
