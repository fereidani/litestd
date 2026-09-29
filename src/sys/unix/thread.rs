//! Threads on top of pthreads.

#[cfg(any(target_os = "linux", target_os = "android"))]
mod cgroup;

#[cfg(not(target_os = "wasi"))]
use core::{cmp, mem};
use core::{
    ffi::c_void,
    mem::{ManuallyDrop, MaybeUninit},
    num::NonZero,
    ptr::{self, NonNull},
    time::Duration,
};

use crate::io;

/// The header of the block a new thread receives: the first field of the
/// spawner's allocation, which `main` is called with.
#[repr(C)]
pub(crate) struct Start {
    pub(crate) main: unsafe fn(NonNull<Self>),
}

/// An owned native thread: `join` waits for it, dropping detaches it.
pub(crate) struct Thread {
    id: libc::pthread_t,
}

// SAFETY: a `pthread_t` names the thread process-wide; every pthread
// function that takes it may be called from any thread.
unsafe impl Send for Thread {}
// SAFETY: shared references give no access to the id at all.
unsafe impl Sync for Thread {}

impl Thread {
    /// Starts a thread with a stack of at least `stack_size` bytes that
    /// calls `(start.main)(start)`.
    ///
    /// # Safety
    ///
    /// `start` must stay valid until the new thread has called `main`, and
    /// that call must be sound. On error, `main` is never called.
    pub(crate) unsafe fn new(
        stack_size: usize,
        start: NonNull<Start>,
    ) -> io::Result<Self> {
        let mut id = MaybeUninit::<libc::pthread_t>::uninit();
        // SAFETY: `id` is valid for writes, `thread_start` has the signature
        // pthreads expect, and the caller vouches for `start`.
        let r = unsafe {
            create(id.as_mut_ptr(), stack_size, start.as_ptr().cast())
        };
        if r != 0 {
            return Err(io::Error::from_raw_os_error(r));
        }
        // SAFETY: `pthread_create` succeeded, so it stored the id.
        let id = unsafe { id.assume_init() };
        Ok(Self { id })
    }

    /// Blocks until the thread and its thread-local destructors have finished.
    /// Fails with `EDEADLK` if the thread is the calling thread.
    pub(crate) fn join(self) -> io::Result<()> {
        // Joined threads must not be detached as well.
        let id = ManuallyDrop::new(self).id;
        // glibc reports a thread that joins itself, but musl waits forever.
        // SAFETY: `pthread_self` has no preconditions.
        if id == unsafe { libc::pthread_self() } {
            return Err(io::Error::from_raw_os_error(libc::EDEADLK));
        }
        // SAFETY: `id` names a thread that was neither joined nor detached.
        let r = unsafe { libc::pthread_join(id, ptr::null_mut()) };
        if r == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(r))
        }
    }

    /// Returns the thread's id; the thread stays owned by `self`. For
    /// `JoinHandleExt`, which WASI lacks.
    #[cfg(unix)]
    pub(crate) const fn as_raw(&self) -> libc::pthread_t {
        self.id
    }

    /// Returns the thread's id without joining or detaching the thread; the
    /// caller must do one of the two.
    #[cfg(unix)]
    pub(crate) fn into_raw(self) -> libc::pthread_t {
        ManuallyDrop::new(self).id
    }
}

impl Drop for Thread {
    fn drop(&mut self) {
        // SAFETY: `id` names a thread that was neither joined nor detached.
        let r = unsafe { libc::pthread_detach(self.id) };
        // Only an invalid or already detached id makes this fail.
        debug_assert_eq!(r, 0);
    }
}

/// Calls `pthread_create` with a stack of at least `stack_size` bytes.
///
/// # Safety
///
/// As for `pthread_create` with `thread_start` as the start routine.
#[cfg(not(miri))]
unsafe fn create(
    id: *mut libc::pthread_t,
    stack_size: usize,
    arg: *mut c_void,
) -> i32 {
    let mut attr = MaybeUninit::<libc::pthread_attr_t>::uninit();
    // SAFETY: `attr` is valid for writes.
    let r = unsafe { libc::pthread_attr_init(attr.as_mut_ptr()) };
    if r != 0 {
        return r;
    }
    let r = stack(stack_size).map_or(libc::EINVAL, |size| {
        // SAFETY: `attr` was initialized above.
        unsafe { libc::pthread_attr_setstacksize(attr.as_mut_ptr(), size) }
    });
    let r = if r == 0 {
        // SAFETY: `attr` is initialized, and the caller vouches for the rest.
        unsafe { libc::pthread_create(id, attr.as_ptr(), thread_start, arg) }
    } else {
        r
    };
    // SAFETY: `attr` was initialized above and is not used again.
    let destroyed = unsafe { libc::pthread_attr_destroy(attr.as_mut_ptr()) };
    // Destroying an initialized attribute object cannot fail.
    debug_assert_eq!(destroyed, 0);
    r
}

/// Calls `pthread_create` with default attributes: Miri supports them only
/// for std, and does not model stack sizes.
///
/// # Safety
///
/// As for `pthread_create` with `thread_start` as the start routine.
#[cfg(miri)]
unsafe fn create(
    id: *mut libc::pthread_t,
    _stack_size: usize,
    arg: *mut c_void,
) -> i32 {
    // SAFETY: the caller vouches for the arguments.
    unsafe { libc::pthread_create(id, ptr::null(), thread_start, arg) }
}

/// Rounds a requested stack size up to at least `PTHREAD_STACK_MIN` and to
/// a whole number of pages, or returns `None` if that overflows.
#[cfg(not(miri))]
fn stack(size: usize) -> Option<usize> {
    // SAFETY: `sysconf` has no preconditions.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    // Page sizes are powers of two; fall back to the smallest one in use.
    let page = usize::try_from(page)
        .ok()
        .filter(|page| page.is_power_of_two())
        .unwrap_or(4096);
    size.max(min_stack()).checked_next_multiple_of(page)
}

/// The smallest stack the system allows a thread.
#[cfg(all(not(miri), not(target_os = "netbsd")))]
const fn min_stack() -> usize {
    libc::PTHREAD_STACK_MIN
}

/// The smallest stack the system allows a thread, which NetBSD reports only
/// through `sysconf`; std guesses 2048 bytes if that fails, and so does
/// litestd.
#[cfg(all(not(miri), target_os = "netbsd"))]
fn min_stack() -> usize {
    // SAFETY: `sysconf` has no preconditions.
    let min = unsafe { libc::sysconf(libc::_SC_THREAD_STACK_MIN) };
    usize::try_from(min).unwrap_or(2048)
}

extern "C" fn thread_start(arg: *mut c_void) -> *mut c_void {
    // `Thread::new` always passes a non-null pointer; checking it here keeps
    // the invariant local for one branch per thread.
    let Some(start) = NonNull::new(arg.cast::<Start>()) else {
        super::os::abort();
    };
    // SAFETY: `start` points to a live `Start` whose owner guarantees that
    // calling `main` with it here is sound. A panic that unwinds out of
    // `main` aborts the process at this `extern "C"` boundary.
    unsafe {
        let main = start.as_ref().main;
        main(start);
    }
    ptr::null_mut()
}

/// Returns whether the calling thread is the module's main thread.
#[cfg(target_os = "wasi")]
pub(crate) use crate::sys::wasm::is_main;

/// Returns whether the calling thread is the process's main thread.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn is_main() -> bool {
    // The main thread's id equals the process id. The raw system call works
    // with C libraries that predate the `gettid` wrapper.
    // SAFETY: `gettid` and `getpid` have no preconditions.
    let (tid, pid) =
        unsafe { (libc::syscall(libc::SYS_gettid), libc::getpid()) };
    tid == libc::c_long::from(pid)
}

/// Returns whether the calling thread is the process's main thread.
#[cfg(any(
    all(any(target_vendor = "apple", target_os = "freebsd"), not(miri)),
    target_os = "openbsd",
    target_os = "dragonfly"
))]
pub(crate) fn is_main() -> bool {
    // SAFETY: `pthread_main_np` has no preconditions.
    unsafe { libc::pthread_main_np() == 1 }
}

/// Returns whether the calling thread is the process's main thread, whose
/// LWP is the process's first.
#[cfg(target_os = "netbsd")]
pub(crate) fn is_main() -> bool {
    // SAFETY: `_lwp_self` has no preconditions.
    unsafe { libc::_lwp_self() == 1 }
}

/// Returns whether the calling thread is Miri's main thread, which Miri
/// names `main`: it does not implement `pthread_main_np`.
#[cfg(all(any(target_vendor = "apple", target_os = "freebsd"), miri))]
pub(crate) fn is_main() -> bool {
    let mut name = [0u8; 8];
    // SAFETY: `name` is valid for writes of its length, and the thread is
    // the calling one.
    let r = unsafe {
        libc::pthread_getname_np(
            libc::pthread_self(),
            name.as_mut_ptr().cast(),
            name.len(),
        )
    };
    r == 0 && name.starts_with(b"main\0")
}

/// The longest name the OS keeps for a thread, including the terminating
/// NUL: Linux's `TASK_COMM_LEN`, macOS's `MAXTHREADNAMESIZE`, OpenBSD's
/// `_MAXCOMLEN`, NetBSD's `PTHREAD_MAX_NAMELEN_NP`, and one more than the
/// `MAXCOMLEN` of FreeBSD and DragonFly.
#[cfg(any(target_os = "linux", target_os = "android"))]
const NAME_SIZE: usize = 16;
#[cfg(target_vendor = "apple")]
const NAME_SIZE: usize = libc::MAXTHREADNAMESIZE;
#[cfg(any(target_os = "freebsd", target_os = "dragonfly"))]
const NAME_SIZE: usize = libc::MAXCOMLEN + 1;
#[cfg(target_os = "openbsd")]
const NAME_SIZE: usize = 24;
#[cfg(target_os = "netbsd")]
const NAME_SIZE: usize = 32;

/// Names the calling thread: wasi-libc keeps no names, as in std.
#[cfg(target_os = "wasi")]
pub(crate) const fn set_name(_name: &str) {}

/// Names the calling thread. The OS keeps at most `NAME_SIZE - 1` bytes, so
/// the name is cut at its first NUL and then to whole characters that fit.
#[cfg(not(target_os = "wasi"))]
pub(crate) fn set_name(name: &str) {
    let mut buf = [0u8; NAME_SIZE];
    let mut len = 0;
    // Copies the bytes the OS keeps, up to a NUL, which ends the name.
    for (dst, byte) in buf.iter_mut().zip(name.bytes().take(NAME_SIZE - 1)) {
        if byte == 0 {
            break;
        }
        *dst = byte;
        len += 1;
    }
    // Drops the bytes of a character that the cut split. Runs at most three
    // times: a UTF-8 character is at most four bytes, and index 0 is always
    // a boundary.
    while !name.is_char_boundary(len) {
        len -= 1;
        if let Some(byte) = buf.get_mut(len) {
            *byte = 0;
        }
    }
    // SAFETY: `buf` holds a NUL-terminated string that fits the OS's limit.
    // macOS names only the calling thread, which is the one to name here.
    // NetBSD's name is a format with one argument, here `buf`.
    let r = unsafe {
        #[cfg(any(
            target_os = "linux",
            target_os = "android",
            target_os = "freebsd"
        ))]
        let r =
            libc::pthread_setname_np(libc::pthread_self(), buf.as_ptr().cast());
        #[cfg(target_vendor = "apple")]
        let r = libc::pthread_setname_np(buf.as_ptr().cast());
        #[cfg(any(target_os = "openbsd", target_os = "dragonfly"))]
        let r = {
            libc::pthread_set_name_np(
                libc::pthread_self(),
                buf.as_ptr().cast(),
            );
            0
        };
        #[cfg(target_os = "netbsd")]
        let r = libc::pthread_setname_np(
            libc::pthread_self(),
            c"%s".as_ptr(),
            buf.as_ptr().cast(),
        );
        r
    };
    // Naming the calling thread cannot fail with a name that fits.
    debug_assert_eq!(r, 0);
}

/// Blocks the calling thread for at least `dur`, to a deadline: WASI
/// runtimes may end a sleep early.
#[cfg(target_os = "wasi")]
pub(crate) fn sleep(dur: Duration) {
    let time = super::time::monotonic();
    super::time::sleep_until(time.saturating_add(dur));
}

/// The longest sleep request, in seconds.
// `time_t::MAX` is positive, so the cast is exact. The libc crate
// deprecates `time_t` on musl ahead of a width change, handled either way.
#[cfg(not(target_os = "wasi"))]
#[allow(clippy::cast_sign_loss, deprecated)]
const MAX_SLEEP_SECS: u64 = libc::time_t::MAX as u64;

/// Blocks the calling thread for at least `dur`.
#[cfg(not(target_os = "wasi"))]
pub(crate) fn sleep(dur: Duration) {
    let mut secs = dur.as_secs();
    let mut nanos = dur.subsec_nanos();
    // Each round sleeps at most `MAX_SLEEP_SECS` seconds. After a signal,
    // the kernel reports the time left and the next round sleeps for that;
    // it never grows, so the loop ends.
    while secs > 0 || nanos > 0 {
        let chunk = cmp::min(secs, MAX_SLEEP_SECS);
        secs -= chunk;
        let mut ts = timespec(chunk, nanos);
        let r = sleep_once(&mut ts);
        if r == libc::EINTR {
            // The time left is at most the request, so this cannot wrap.
            secs += u64::try_from(ts.tv_sec).unwrap_or(0);
            nanos = u32::try_from(ts.tv_nsec).unwrap_or(0);
        } else {
            // Every other error means an invalid request, which `timespec`
            // rules out.
            debug_assert_eq!(r, 0);
            nanos = 0;
        }
    }
}

/// Sleeps for the time in `ts`, returning 0 or the error code: `EINTR` if a
/// signal handler ran first, with the time left stored back in `ts`.
#[cfg(not(target_os = "wasi"))]
fn sleep_once(ts: &mut libc::timespec) -> libc::c_int {
    // One pointer for both arguments: the call reads the request through it
    // and writes the time left back.
    let ts = ptr::from_mut(ts);
    // SAFETY: `ts` is valid for reads and writes of a `timespec`.
    #[cfg(not(any(target_vendor = "apple", target_os = "openbsd")))]
    let r = unsafe { libc::clock_nanosleep(libc::CLOCK_MONOTONIC, 0, ts, ts) };
    // macOS and OpenBSD have no `clock_nanosleep`; their `nanosleep`, which
    // std uses too, waits on a clock that setting the time does not move.
    // SAFETY: as above.
    #[cfg(any(target_vendor = "apple", target_os = "openbsd"))]
    let r = match unsafe { libc::nanosleep(ts, ts) } {
        0 => 0,
        _ => super::os::errno(),
    };
    r
}

/// Builds a `timespec` from at most `MAX_SLEEP_SECS` seconds and less than
/// a second of nanoseconds.
// See `MAX_SLEEP_SECS` about `time_t`.
#[cfg(not(target_os = "wasi"))]
#[allow(deprecated)]
fn timespec(secs: u64, nanos: u32) -> libc::timespec {
    // SAFETY: `timespec` holds only integers, and all zeros is valid. Some
    // targets add private padding fields, which rules out a struct literal.
    let mut ts: libc::timespec = unsafe { mem::zeroed() };
    ts.tv_sec = libc::time_t::try_from(secs).unwrap_or(libc::time_t::MAX);
    // `tv_nsec` is 32 bits wide on some targets, but `nanos` fits anyway.
    #[allow(clippy::unnecessary_fallible_conversions)]
    let nanos = nanos.try_into().unwrap_or(0);
    ts.tv_nsec = nanos;
    ts
}

/// Offers the rest of the calling thread's time slice to other threads.
pub(crate) fn yield_now() {
    // SAFETY: `sched_yield` has no preconditions.
    let r = unsafe { libc::sched_yield() };
    // `sched_yield` always succeeds on Linux and macOS.
    debug_assert_eq!(r, 0);
}

/// Returns the number of CPUs the calling thread may run on; on Linux, as in
/// std, at most the whole CPUs of the process's cgroup quota.
pub(crate) fn available_parallelism() -> io::Result<NonZero<usize>> {
    let count = cpu_count()?;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let count = count.min(cgroup::quota());
    Ok(count)
}

/// Counts the CPUs the calling thread may run on: those of its affinity
/// mask where the OS reports one, else those online.
fn cpu_count() -> io::Result<NonZero<usize>> {
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd"
    ))]
    if let Some(count) = affinity_count() {
        return Ok(count);
    }
    // SAFETY: `sysconf` has no preconditions.
    match unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) } {
        -1 => Err(io::Error::last_os_error()),
        count => usize::try_from(count)
            .ok()
            .and_then(NonZero::new)
            .ok_or_else(unknown_parallelism),
    }
}

/// Counts the CPUs in the calling thread's affinity mask, or returns `None`
/// if it cannot be read (more CPUs than `cpu_set_t` holds) or is empty, as
/// some old kernels report it.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn affinity_count() -> Option<NonZero<usize>> {
    // SAFETY: `cpu_set_t` is a plain bit mask, for which zero is valid.
    let mut set: libc::cpu_set_t = unsafe { mem::zeroed() };
    // SAFETY: `set` is valid for writes of the size passed.
    let r = unsafe {
        libc::sched_getaffinity(0, size_of::<libc::cpu_set_t>(), &raw mut set)
    };
    if r != 0 {
        return None;
    }
    // SAFETY: `set` was initialized above.
    let count = unsafe { libc::CPU_COUNT(&set) };
    usize::try_from(count).ok().and_then(NonZero::new)
}

/// Counts the CPUs in the calling process's affinity mask, or returns
/// `None` if it cannot be read or is empty.
#[cfg(target_os = "freebsd")]
fn affinity_count() -> Option<NonZero<usize>> {
    // SAFETY: `cpuset_t` is a plain bit mask, for which zero is valid.
    let mut set: libc::cpuset_t = unsafe { mem::zeroed() };
    // SAFETY: `set` is valid for writes of the size passed; the id -1 names
    // the calling process.
    let r = unsafe {
        libc::cpuset_getaffinity(
            libc::CPU_LEVEL_WHICH,
            libc::CPU_WHICH_PID,
            -1,
            size_of::<libc::cpuset_t>(),
            &raw mut set,
        )
    };
    if r != 0 {
        return None;
    }
    // SAFETY: `set` was initialized above.
    let count = unsafe { libc::CPU_COUNT(&set) };
    usize::try_from(count).ok().and_then(NonZero::new)
}

#[cold]
const fn unknown_parallelism() -> io::Error {
    io::const_error!(
        io::ErrorKind::NotFound,
        "the number of hardware threads is not known for the target platform"
    )
}
