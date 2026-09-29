//! The process environment through the C library: `environ` and the
//! `getenv` family behind a read-write lock, the arguments from the `argv`
//! that glibc hands to litestd's initializer or else `/proc/self/cmdline` on
//! Linux, from `argv` on macOS, from `sysctl` on the BSDs and from WASI's
//! `args_get`, and the current, temporary and home directories.

#[cfg(any(
    target_vendor = "apple",
    all(target_os = "linux", target_env = "gnu", not(miri))
))]
use core::ffi::c_char;
#[cfg(not(any(target_vendor = "apple", target_os = "wasi")))]
use core::ffi::c_int;
#[cfg(all(unix, not(target_os = "android")))]
use core::mem::MaybeUninit;
#[cfg(all(target_os = "linux", target_env = "gnu", not(miri)))]
use core::sync::atomic::AtomicUsize;
use core::{
    ffi::CStr,
    marker::PhantomData,
    ptr, str,
    sync::atomic::{AtomicPtr, AtomicU8, Ordering},
};

use alloc_crate::{borrow::Cow, boxed::Box, vec::Vec};

use super::os::{self, STACK_BUF, cstr, cvt, path as sys_path};
use crate::{
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
    sync::raw_rwlock::RawRwLock,
};

/// A unit of the strings the OS hands out.
pub(crate) type Unit = u8;

/// std's `ENV_LOCK`: litestd's readers of the environment share it, and
/// `set_var` and `remove_var` hold it alone.
static ENV_LOCK: RawRwLock = RawRwLock::new();

/// Holds `ENV_LOCK` shared until dropped, on the thread that took it.
pub(crate) struct EnvGuard(PhantomData<*const ()>);

impl EnvGuard {
    /// Takes the lock shared, blocking while `set_var` or `remove_var` runs.
    pub(crate) fn read() -> Self {
        ENV_LOCK.read();
        Self(PhantomData)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: the guard holds the read lock that `read` took.
        unsafe { ENV_LOCK.read_unlock() };
    }
}

/// Holds `ENV_LOCK` alone until dropped, on the thread that took it.
struct EnvWriteGuard(PhantomData<*const ()>);

impl EnvWriteGuard {
    /// Takes the lock alone, blocking while any other thread holds it.
    fn new() -> Self {
        ENV_LOCK.write();
        Self(PhantomData)
    }
}

impl Drop for EnvWriteGuard {
    fn drop(&mut self) {
        // SAFETY: the guard holds the write lock that `new` took.
        unsafe { ENV_LOCK.write_unlock() };
    }
}

/// Copies units the OS handed out into an `OsString`.
pub(crate) fn os_string(units: &[u8]) -> OsString {
    OsString::from_unix_vec(units.to_vec())
}

/// The index of the first NUL in `units`. The lists of strings end with a
/// NUL, which lets the C library's vectorized `strlen` search them.
pub(crate) fn find_nul(units: &[u8]) -> Option<usize> {
    if units.last() == Some(&0) {
        // SAFETY: `units` ends with a NUL, so `strlen` stops inside it.
        return Some(unsafe { libc::strlen(units.as_ptr().cast()) });
    }
    units.iter().position(|&unit| unit == 0)
}

/// The arguments as [`args`] returns them.
struct Cmdline {
    bytes: Vec<u8>,
    count: usize,
    /// Whether `bytes` is UTF-8: [`UNCHECKED`] until [`args_text`] checks.
    utf8: AtomicU8,
}

/// [`Cmdline::utf8`] before the check.
const UNCHECKED: u8 = 0;
/// [`Cmdline::utf8`] once the arguments proved to be UTF-8.
const VALID: u8 = 1;
/// [`Cmdline::utf8`] once an argument proved not to be UTF-8.
const INVALID: u8 = 2;

/// The arguments once read: null, or a leaked `Cmdline` whose bytes are
/// never written and which is never freed.
static CMDLINE: AtomicPtr<Cmdline> = AtomicPtr::new(ptr::null_mut());

/// The `argv` and `argc` that glibc passed to litestd's initializer, as std
/// keeps them: `argv` and its strings live as long as the process. `ARGV`
/// stays null if the initializer did not run.
#[cfg(all(target_os = "linux", target_env = "gnu", not(miri)))]
static ARGV: AtomicPtr<*const c_char> = AtomicPtr::new(ptr::null_mut());
#[cfg(all(target_os = "linux", target_env = "gnu", not(miri)))]
static ARGC: AtomicUsize = AtomicUsize::new(0);

/// Keeps the arguments that glibc passes to initializers, for [`args`].
#[cfg(all(target_os = "linux", target_env = "gnu", not(miri)))]
#[allow(clippy::similar_names, reason = "C's names")]
pub(crate) fn save_args(argc: c_int, argv: *const *const c_char) {
    ARGC.store(usize::try_from(argc).unwrap_or(0), Ordering::Relaxed);
    // `Release` publishes `ARGC` with `ARGV`, for a library that the loader
    // initializes while other threads run.
    ARGV.store(argv.cast_mut(), Ordering::Release);
}

/// Returns the arguments, each followed by a NUL, and their count. The first
/// call reads them: on Linux from the `argv` that glibc gave the
/// initializer, or else from `/proc/self/cmdline`, where they are empty if
/// the file cannot be read, and from `argv` on macOS; later calls share that
/// copy.
pub(crate) fn args() -> (Cow<'static, [u8]>, usize) {
    let cmdline = cmdline().unwrap_or_else(read_cmdline);
    (Cow::Borrowed(&cmdline.bytes), cmdline.count)
}

/// The arguments that [`args`] shares, as one `str` if they are UTF-8, as
/// arguments normally are: checked once, then remembered, so that
/// `env::args` need not check each one.
pub(crate) fn args_text() -> Option<&'static str> {
    let cmdline = cmdline()?;
    // The flag only says what any check of the unchanging bytes finds, so
    // racing checks agree and `Relaxed` suffices.
    let utf8 = match cmdline.utf8.load(Ordering::Relaxed) {
        UNCHECKED => {
            let utf8 = if str::from_utf8(&cmdline.bytes).is_ok() {
                VALID
            } else {
                INVALID
            };
            cmdline.utf8.store(utf8, Ordering::Relaxed);
            utf8
        }
        utf8 => utf8,
    };
    (utf8 == VALID).then(|| {
        // SAFETY: `VALID` is stored only after `bytes`, which nothing writes,
        // proved to be UTF-8.
        unsafe { str::from_utf8_unchecked(&cmdline.bytes) }
    })
}

/// The arguments, if they have been read.
fn cmdline() -> Option<&'static Cmdline> {
    // `Acquire` pairs with the `Release` that published the copy.
    let published = CMDLINE.load(Ordering::Acquire);
    // SAFETY: `CMDLINE` holds null or a leaked `Cmdline`, which is never
    // freed and whose `bytes` are never written.
    unsafe { published.as_ref() }
}

/// Reads the arguments into `CMDLINE`, unless another thread got there
/// first, and returns the copy published.
#[cold]
#[inline(never)]
fn read_cmdline() -> &'static Cmdline {
    let mut bytes = Vec::new();
    read_args(&mut bytes);
    // A process may overwrite the NUL that ends its last argument.
    if bytes.last().is_some_and(|&byte| byte != 0) {
        bytes.push(0);
    }
    let mut count = 0;
    let mut rest = &bytes[..];
    // Each pass drops at least the NUL it found.
    while let Some(nul) = find_nul(rest) {
        count += 1;
        rest = rest.get(nul + 1..).unwrap_or_default();
    }
    let utf8 = AtomicU8::new(UNCHECKED);
    let new = Box::into_raw(Box::new(Cmdline { bytes, count, utf8 }));
    // `Release` publishes the copy, and `Acquire` takes the one published
    // first.
    let published = match CMDLINE.compare_exchange(
        ptr::null_mut(),
        new,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => new,
        Err(published) => {
            // SAFETY: `new` came from `Box::into_raw` above and was never
            // shared.
            drop(unsafe { Box::from_raw(new) });
            published
        }
    };
    // SAFETY: `published` is the non-null pointer in `CMDLINE`: a leaked
    // `Cmdline`, which is never freed and whose `bytes` are never written.
    unsafe { &*published }
}

/// Appends the arguments, each followed by a NUL but perhaps the last, to
/// `bytes`: from the `argv` that glibc gave the initializer, as std reads
/// them there, or else the contents of `/proc/self/cmdline`, or nothing if
/// the file cannot be read.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn read_args(bytes: &mut Vec<u8>) {
    #[cfg(all(target_env = "gnu", not(miri)))]
    {
        // `Acquire` pairs with the `Release` in `save_args`.
        let list = ARGV.load(Ordering::Acquire);
        if !list.is_null() {
            let count = ARGC.load(Ordering::Relaxed);
            // SAFETY: glibc's `argv` holds `argc` entries, then a null one,
            // each a C string or null, for the life of the process.
            unsafe { append_argv(bytes, count, list.cast_const()) };
            return;
        }
    }
    // SAFETY: the path is a C string. The descriptor is closed below, and
    // close-on-exec keeps it from leaking into a child meanwhile.
    let fd = unsafe {
        libc::open(
            c"/proc/self/cmdline".as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC,
        )
    };
    if fd >= 0 {
        if !read_to_end(fd, bytes) {
            bytes.clear();
        }
        // SAFETY: `fd` is open and owned here, and not used afterwards.
        let r = unsafe { libc::close(fd) };
        debug_assert_eq!(r, 0);
    }
}

/// Appends the arguments, each followed by a NUL, to `bytes`, as std reads
/// them: from the `argv` that dyld keeps.
#[cfg(target_vendor = "apple")]
fn read_args(bytes: &mut Vec<u8>) {
    // SAFETY: both functions return the addresses of variables that dyld
    // sets before `main` and a program changes, if at all, before it starts
    // threads, as std assumes too.
    let (count, list) =
        unsafe { (libc::_NSGetArgc().read(), libc::_NSGetArgv().read()) };
    if list.is_null() {
        return;
    }
    let count = usize::try_from(count).unwrap_or(0);
    // SAFETY: dyld's `argv` holds `argc` entries, then a null one, each a C
    // string or null.
    unsafe { append_argv(bytes, count, list.cast_const().cast()) };
}

/// Appends the strings of `list`, a C `argv`, each with its NUL, to `bytes`:
/// up to `count` of them or the first null entry, which some argument
/// parsers leave behind.
///
/// # Safety
///
/// `list` holds `count` entries, then a null one, each a C string or null,
/// unchanged during the call.
#[cfg(any(
    target_vendor = "apple",
    all(target_os = "linux", target_env = "gnu", not(miri))
))]
unsafe fn append_argv(
    bytes: &mut Vec<u8>,
    count: usize,
    list: *const *const c_char,
) {
    for i in 0..count {
        // SAFETY: the caller vouches for `count` entries.
        let arg = unsafe { list.add(i).read() };
        if arg.is_null() {
            break;
        }
        // SAFETY: the entries that are not null are C strings.
        let arg = unsafe { CStr::from_ptr(arg) };
        bytes.extend_from_slice(arg.to_bytes_with_nul());
    }
}

/// Appends the arguments, each followed by a NUL but perhaps the last, to
/// `bytes`, as `sysctl` reports them: consecutive C strings on FreeBSD and
/// DragonFly (`KERN_PROC_ARGS`) and on NetBSD (`KERN_PROC_ARGV`), or nothing
/// if it fails.
#[cfg(any(
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
fn read_args(bytes: &mut Vec<u8>) {
    // SAFETY: `getpid` has no preconditions.
    let pid = unsafe { libc::getpid() };
    #[cfg(any(target_os = "freebsd", target_os = "dragonfly"))]
    let mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_ARGS, pid];
    #[cfg(target_os = "netbsd")]
    let mib = [
        libc::CTL_KERN,
        libc::KERN_PROC_ARGS,
        pid,
        libc::KERN_PROC_ARGV,
    ];
    if sysctl_into(&mib, bytes).is_err() {
        bytes.clear();
    }
}

/// Appends the arguments, each followed by a NUL, to `bytes`, or nothing if
/// `sysctl` fails.
#[cfg(target_os = "openbsd")]
fn read_args(bytes: &mut Vec<u8>) {
    if openbsd_args(bytes).is_err() {
        bytes.clear();
    }
}

/// Appends the arguments to `bytes`, each followed by a NUL, from OpenBSD's
/// `KERN_PROC_ARGV`: a vector of pointers, up to a null one, to C strings
/// that the kernel places after it in the same buffer. The pointers become
/// offsets into the buffer, and one that points outside it ends the list.
#[cfg(target_os = "openbsd")]
fn openbsd_args(bytes: &mut Vec<u8>) -> io::Result<()> {
    // SAFETY: `getpid` has no preconditions.
    let pid = unsafe { libc::getpid() };
    let mib = [
        libc::CTL_KERN,
        libc::KERN_PROC_ARGS,
        pid,
        libc::KERN_PROC_ARGV,
    ];
    let mut buf = Vec::new();
    sysctl_into(&mib, &mut buf)?;
    let base = buf.as_ptr().addr();
    for slot in buf.chunks_exact(size_of::<usize>()) {
        let mut word = [0; size_of::<usize>()];
        word.copy_from_slice(slot);
        let arg = usize::from_ne_bytes(word)
            .checked_sub(base)
            .and_then(|offset| buf.get(offset..))
            .and_then(|rest| CStr::from_bytes_until_nul(rest).ok());
        let Some(arg) = arg else { break };
        bytes.extend_from_slice(arg.to_bytes_with_nul());
    }
    Ok(())
}

/// Appends the arguments, each followed by a NUL, to `bytes`, as WASI's
/// `args_get` writes them, or nothing if it fails.
#[cfg(target_os = "wasi")]
#[allow(clippy::similar_names, reason = "WASI's names")]
fn read_args(bytes: &mut Vec<u8>) {
    // The WASI calls, which return an error number, 0 for success.
    #[link(wasm_import_module = "wasi_snapshot_preview1")]
    unsafe extern "C" {
        fn args_sizes_get(argc: *mut usize, size: *mut usize) -> i32;
        fn args_get(argv: *mut *mut u8, buf: *mut u8) -> i32;
    }
    let (mut argc, mut size) = (0, 0);
    // SAFETY: both pointers are valid for writes of a `usize`.
    if unsafe { args_sizes_get(&raw mut argc, &raw mut size) } != 0 {
        return;
    }
    // `args_get` also stores a pointer to each argument, unused here.
    let mut argv = Vec::<*mut u8>::with_capacity(argc);
    let len = bytes.len();
    bytes.reserve_exact(size);
    let buf = bytes.spare_capacity_mut().as_mut_ptr().cast();
    // SAFETY: `argv` has room for `argc` pointers and `buf` for `size`
    // bytes, the sizes WASI reported for these writes.
    if unsafe { args_get(argv.as_mut_ptr(), buf) } == 0 {
        // SAFETY: `args_get` initialized the `size` bytes after `len`.
        unsafe { bytes.set_len(len + size) };
    }
}

/// Reads the `sysctl` value `mib` into `buf`, sized as the kernel reports.
/// A value that grows between the size query and the read fails the read
/// with `ENOMEM`; a few retries outlast any such race.
#[cfg(any(
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
fn sysctl_into(mib: &[c_int], buf: &mut Vec<u8>) -> io::Result<()> {
    #[allow(clippy::cast_possible_truncation, reason = "four entries")]
    let namelen = mib.len() as libc::c_uint;
    for _ in 0..8 {
        let mut len = 0;
        // SAFETY: `mib` holds `namelen` entries; a null buffer asks for the
        // size, which `len` receives.
        cvt(unsafe {
            libc::sysctl(
                mib.as_ptr(),
                namelen,
                ptr::null_mut(),
                &raw mut len,
                ptr::null_mut(),
                0,
            )
        })?;
        buf.clear();
        buf.reserve(len);
        let mut got = buf.capacity();
        // SAFETY: `buf` has room for `got` bytes, the most the kernel
        // writes, and `got` receives how many it wrote.
        let r = unsafe {
            libc::sysctl(
                mib.as_ptr(),
                namelen,
                buf.as_mut_ptr().cast(),
                &raw mut got,
                ptr::null_mut(),
                0,
            )
        };
        match cvt(r) {
            Ok(_) => {
                // SAFETY: the kernel initialized the first `got` bytes, no
                // more than the room it was given.
                unsafe { buf.set_len(got.min(buf.capacity())) };
                return Ok(());
            }
            Err(e) if e.raw_os_error() == Some(libc::ENOMEM) => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::from_raw_os_error(libc::ENOMEM))
}

/// Reads `fd` to its end into `buf`. Returns `false` on an error.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn read_to_end(fd: c_int, buf: &mut Vec<u8>) -> bool {
    let mut len = 0;
    // Each pass reads at least one byte of the finite file or returns. A
    // read of `/proc` is interrupted only when the process is being killed,
    // so every error ends the loop.
    loop {
        if len == buf.len() {
            buf.resize(len.max(256) * 2, 0);
        }
        let spare = buf.get_mut(len..).unwrap_or_default();
        // SAFETY: `spare` is valid for writes of its length.
        let n =
            unsafe { libc::read(fd, spare.as_mut_ptr().cast(), spare.len()) };
        match usize::try_from(n) {
            Ok(0) => {
                buf.truncate(len);
                return true;
            }
            Ok(n) => len += n.min(spare.len()),
            Err(_) => return false,
        }
    }
}

/// A snapshot of the environment: each `KEY=VALUE` entry, followed by a
/// NUL, and their count. Entries without a `=` after their first byte are
/// left out, as std leaves them out.
pub(crate) fn vars() -> (Vec<u8>, usize) {
    let guard = EnvGuard::read();
    // SAFETY: litestd changes the environment only under the write lock,
    // which the guard excludes, and `set_var`'s contract rules out other
    // writers.
    let entries = unsafe { os::Entries::new(&guard) };
    let len = entries.clone().map(|entry| entry.len() + 1).sum();
    let mut out = Vec::with_capacity(len);
    let mut count = 0;
    for entry in entries {
        if entry.get(1..).is_some_and(|rest| rest.contains(&b'=')) {
            out.extend_from_slice(entry);
            out.push(0);
            count += 1;
        }
    }
    (out, count)
}

/// Returns the value of the variable `key`, or `None` if it is not set.
pub(crate) fn getenv(key: &OsStr) -> Option<OsString> {
    getenv_bytes(key.as_encoded_bytes())
}

/// Looks `key` up with the C library's `getenv` under the read lock, as std
/// does. A name containing NUL matches no entry.
fn getenv_bytes(key: &[u8]) -> Option<OsString> {
    let mut buf = STACK_BUF;
    let key = cstr(key, &mut buf)?;
    let _guard = EnvGuard::read();
    // SAFETY: `key` is a C string. The read lock keeps litestd's writers out
    // until the value is copied, and `set_var`'s contract all others.
    let value = unsafe { libc::getenv(key.as_ptr()) };
    // SAFETY: a result that is not null is a C string of the environment,
    // unchanged while the lock is held.
    (!value.is_null())
        .then(|| os_string(unsafe { CStr::from_ptr(value) }.to_bytes()))
}

/// Sets `key` to `value`. Returns `false` if the C library refuses the
/// pair or either contains a NUL.
///
/// # Safety
///
/// No other thread may access the environment other than through
/// litestd's `env` meanwhile, as `env::set_var` requires.
pub(crate) unsafe fn setenv(key: &OsStr, value: &OsStr) -> bool {
    let (mut key_buf, mut value_buf) = (STACK_BUF, STACK_BUF);
    let key = cstr(key.as_encoded_bytes(), &mut key_buf);
    let value = cstr(value.as_encoded_bytes(), &mut value_buf);
    let (Some(key), Some(value)) = (key, value) else {
        return false;
    };
    let _guard = EnvWriteGuard::new();
    // SAFETY: both are C strings. The write lock excludes litestd's readers
    // and the caller all other access.
    unsafe { libc::setenv(key.as_ptr(), value.as_ptr(), 1) == 0 }
}

/// Removes `key`. Returns `false` if the C library refuses the name or it
/// contains a NUL.
///
/// # Safety
///
/// As for [`setenv`].
pub(crate) unsafe fn unsetenv(key: &OsStr) -> bool {
    let mut buf = STACK_BUF;
    let Some(key) = cstr(key.as_encoded_bytes(), &mut buf) else {
        return false;
    };
    let _guard = EnvWriteGuard::new();
    // SAFETY: as in `setenv`.
    unsafe { libc::unsetenv(key.as_ptr()) == 0 }
}

/// Panics as std does: WASI has no temporary directory.
#[cfg(target_os = "wasi")]
#[cold]
#[track_caller]
#[allow(clippy::panic, reason = "std panics here")]
pub(crate) fn temp_dir() -> PathBuf {
    panic!("not supported by WASI yet")
}

/// `TMPDIR`, or the system's temporary directory.
#[cfg(unix)]
pub(crate) fn temp_dir() -> PathBuf {
    getenv_bytes(b"TMPDIR").map_or_else(default_temp_dir, PathBuf::from)
}

/// The temporary directory without `TMPDIR`. Android has no shared one, and
/// std falls back to `/data/local/tmp` there.
#[cfg(not(any(target_vendor = "apple", target_os = "wasi")))]
fn default_temp_dir() -> PathBuf {
    PathBuf::from(if cfg!(target_os = "android") {
        "/data/local/tmp"
    } else {
        "/tmp"
    })
}

/// The temporary directory without `TMPDIR`: the user's own, as std reads it
/// with `confstr`, or `/tmp` if that fails.
#[cfg(target_vendor = "apple")]
#[cold]
fn default_temp_dir() -> PathBuf {
    sys_path::fill_path(&mut |buf| {
        // SAFETY: `buf` is valid for writes of its length.
        let size = unsafe {
            libc::confstr(
                libc::_CS_DARWIN_USER_TEMP_DIR,
                buf.as_mut_ptr().cast(),
                buf.len(),
            )
        };
        // The result counts the NUL; a larger one did not fit.
        match size {
            0 => Err(io::Error::last_os_error()),
            size if size <= buf.len() => Ok(Some(size - 1)),
            _ => Ok(None),
        }
    })
    .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// None: std reads no home directory on WASI.
#[cfg(target_os = "wasi")]
#[allow(clippy::missing_const_for_fn, reason = "stands in for an OS call")]
pub(crate) fn home_dir() -> Option<PathBuf> {
    None
}

/// `HOME` unless empty, then the user database's entry.
#[cfg(unix)]
pub(crate) fn home_dir() -> Option<PathBuf> {
    getenv_bytes(b"HOME")
        .filter(|home| !home.is_empty())
        .or_else(passwd_home)
        .map(PathBuf::from)
}

/// Android has no user database for std to consult.
#[cfg(target_os = "android")]
const fn passwd_home() -> Option<OsString> {
    None
}

/// The home directory in the user database's entry for the real user id.
#[cfg(all(unix, not(target_os = "android")))]
fn passwd_home() -> Option<OsString> {
    /// The largest buffer offered for the entry's strings.
    const MAX_BUF: usize = 1 << 20;
    // SAFETY: `sysconf` has no preconditions.
    let hint = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    let len = usize::try_from(hint).unwrap_or(0).clamp(512, MAX_BUF);
    let mut buf = alloc_crate::vec![0_u8; len];
    // Each pass returns or doubles the buffer, up to `MAX_BUF`.
    loop {
        let mut entry = MaybeUninit::<libc::passwd>::uninit();
        let mut found = ptr::null_mut();
        // SAFETY: `entry` and `found` are valid for writes, and `buf` for
        // writes of its length.
        let r = unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                entry.as_mut_ptr(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                &raw mut found,
            )
        };
        if r == 0 {
            if found.is_null() {
                return None;
            }
            debug_assert_eq!(found.cast_const(), entry.as_ptr());
            // SAFETY: a result found means that `getpwuid_r` filled `entry`.
            let dir = unsafe { entry.assume_init_ref() }.pw_dir;
            if dir.is_null() {
                return None;
            }
            // SAFETY: `pw_dir` is a C string in `buf`, which is still live.
            return Some(os_string(unsafe { CStr::from_ptr(dir) }.to_bytes()));
        }
        // std gives up on a short buffer; retrying finds long entries too.
        if r != libc::ERANGE || buf.len() >= MAX_BUF {
            return None;
        }
        buf.resize(buf.len() * 2, 0);
    }
}

pub(crate) use sys_path::getcwd;

pub(crate) fn chdir(path: &Path) -> io::Result<()> {
    let mut buf = STACK_BUF;
    let path = sys_path::path_cstr(path, &mut buf)?;
    // SAFETY: `path` is a C string.
    cvt(unsafe { libc::chdir(path.as_ptr()) }).map(drop)
}

/// The path of the executable that dyld records, which std returns as it is.
#[cfg(target_vendor = "apple")]
pub(crate) fn current_exe() -> io::Result<PathBuf> {
    sys_path::fill_path(&mut |buf| {
        let mut size = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        // SAFETY: `buf` is valid for writes of `size` bytes. The libc crate
        // deprecates the binding in favor of the `mach2` crate.
        #[allow(deprecated)]
        let r = unsafe {
            libc::_NSGetExecutablePath(buf.as_mut_ptr().cast(), &raw mut size)
        };
        if r == 0 {
            let path = CStr::from_bytes_until_nul(buf);
            return Ok(Some(path.map_or(buf.len(), CStr::count_bytes)));
        }
        // A buffer too small fails with the size needed; with no path to
        // report, it fails without one, and std returns `errno` then.
        if usize::try_from(size).is_ok_and(|size| size > buf.len()) {
            Ok(None)
        } else {
            Err(io::Error::last_os_error())
        }
    })
}

/// The path of the executable that the kernel records, as std reads it.
#[cfg(any(target_os = "freebsd", target_os = "dragonfly"))]
pub(crate) fn current_exe() -> io::Result<PathBuf> {
    let mib = [
        libc::CTL_KERN,
        libc::KERN_PROC,
        libc::KERN_PROC_PATHNAME,
        -1,
    ];
    let mut path = Vec::new();
    sysctl_into(&mib, &mut path)?;
    // The kernel counts the NUL; with no path to report, it counts nothing.
    if path.pop().is_none() {
        return Err(io::Error::last_os_error());
    }
    Ok(PathBuf::from(OsString::from_unix_vec(path)))
}

/// The path of the executable that the kernel records, or, as std falls
/// back to, the target of `/proc/curproc/exe` if that is a regular file.
#[cfg(target_os = "netbsd")]
pub(crate) fn current_exe() -> io::Result<PathBuf> {
    let mib = [
        libc::CTL_KERN,
        libc::KERN_PROC_ARGS,
        -1,
        libc::KERN_PROC_PATHNAME,
    ];
    let mut path = Vec::new();
    // The kernel counts the NUL, which needs at least one byte before it.
    if sysctl_into(&mib, &mut path).is_ok() && path.len() > 1 {
        path.pop();
        return Ok(PathBuf::from(OsString::from_unix_vec(path)));
    }
    let link = c"/proc/curproc/exe";
    // SAFETY: `stat` holds only integers, for which zero is valid.
    let mut st: libc::stat = unsafe { core::mem::zeroed() };
    // SAFETY: `link` is a C string and `st` a writable `stat`.
    let r = unsafe { libc::stat(link.as_ptr(), &raw mut st) };
    if r == 0 && st.st_mode & libc::S_IFMT == libc::S_IFREG {
        return sys_path::readlink(link);
    }
    Err(io::const_error!(
        io::ErrorKind::Uncategorized,
        "/proc/curproc/exe doesn't point to regular file.",
    ))
}

/// The first argument, as std reads it on OpenBSD: resolved if it holds a
/// `/`, as a program started by path does, and as it is otherwise, for a
/// program that the shell found in `PATH`.
#[cfg(target_os = "openbsd")]
pub(crate) fn current_exe() -> io::Result<PathBuf> {
    let mut args = Vec::new();
    openbsd_args(&mut args)?;
    let Ok(first) = CStr::from_bytes_until_nul(&args) else {
        return Err(io::const_error!(
            io::ErrorKind::Uncategorized,
            "no current exe available",
        ));
    };
    if first.to_bytes().contains(&b'/') {
        sys_path::realpath(first)
    } else {
        Ok(PathBuf::from(OsString::from_unix_vec(
            first.to_bytes().to_vec(),
        )))
    }
}

/// Fails as in std: a WASI module does not know its file.
#[cfg(target_os = "wasi")]
#[allow(clippy::missing_const_for_fn, reason = "stands in for an OS call")]
pub(crate) fn current_exe() -> io::Result<PathBuf> {
    Err(crate::sys::unsupported::UNSUPPORTED)
}

/// The target of `/proc/self/exe`.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn current_exe() -> io::Result<PathBuf> {
    sys_path::readlink(c"/proc/self/exe").map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            io::const_error!(
                io::ErrorKind::Uncategorized,
                "no /proc/self/exe available. Is /proc mounted?",
            )
        } else {
            error
        }
    })
}
