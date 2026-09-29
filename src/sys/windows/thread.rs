//! Threads on top of `CreateThread`.

use core::{
    ffi::c_void,
    num::NonZero,
    ptr::{self, NonNull},
    sync::atomic::{AtomicU32, Ordering::Relaxed},
    time::Duration,
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_POSSIBLE_DEADLOCK, HANDLE, WAIT_FAILED},
    System::{
        SystemInformation::{GetSystemInfo, SYSTEM_INFO},
        Threading::{
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateThread,
            CreateWaitableTimerExW, GetCurrentThread, GetCurrentThreadId,
            GetThreadId, INFINITE, STACK_SIZE_PARAM_IS_A_RESERVATION,
            SetThreadDescription, SetWaitableTimer, Sleep, SwitchToThread,
            TIMER_ALL_ACCESS, WaitForSingleObject,
        },
    },
};

use super::{os::timeout_ms, time};
use crate::io;

/// `ERROR_POSSIBLE_DEADLOCK` (1131, so the cast is exact) as a raw OS error.
#[allow(clippy::cast_possible_wrap)]
const POSSIBLE_DEADLOCK: i32 = ERROR_POSSIBLE_DEADLOCK as i32;

/// The header of the block a new thread receives: the thread calls `main`
/// with a pointer to it, which is the first field of the spawner's allocation.
#[repr(C)]
pub(crate) struct Start {
    pub(crate) main: unsafe fn(NonNull<Self>),
}

/// An owned native thread: `join` waits for it, dropping detaches it.
pub(crate) struct Thread {
    handle: HANDLE,
}

// SAFETY: a thread handle may be waited on and closed from any thread.
unsafe impl Send for Thread {}
// SAFETY: shared references give no access to the handle at all.
unsafe impl Sync for Thread {}

impl Thread {
    /// Starts a thread with a stack of at least `stack_size` bytes that
    /// calls `(start.main)(start)`.
    ///
    /// # Safety
    ///
    /// `start` must stay valid until `main` runs on the new thread, and that
    /// call must be sound. On error, `main` is never called.
    pub(crate) unsafe fn new(
        stack_size: usize,
        start: NonNull<Start>,
    ) -> io::Result<Self> {
        /// The granularity of reservations, 64 KiB on every Windows version.
        const GRANULARITY: usize = 64 * 1024;
        // Reject sizes that overflow when the system rounds them up to the
        // granularity: Wine asserts on them and Windows may wrap around.
        let Some(stack_size) = stack_size.checked_next_multiple_of(GRANULARITY)
        else {
            return Err(invalid_stack_size());
        };
        // SAFETY: `thread_start` has the signature `CreateThread` expects,
        // and the caller vouches for `start`.
        let handle = unsafe {
            CreateThread(
                ptr::null(),
                stack_size,
                Some(thread_start),
                start.as_ptr().cast::<c_void>(),
                STACK_SIZE_PARAM_IS_A_RESERVATION,
                ptr::null_mut(),
            )
        };
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self { handle })
        }
    }

    /// Blocks until the thread has run its thread-local destructors and
    /// exited; joining the calling thread fails with `ERROR_POSSIBLE_DEADLOCK`.
    pub(crate) fn join(self) -> io::Result<()> {
        // SAFETY: `handle` is an open thread handle with full access, and
        // `GetCurrentThreadId` has no preconditions.
        if unsafe { GetThreadId(self.handle) == GetCurrentThreadId() } {
            return Err(io::Error::from_raw_os_error(POSSIBLE_DEADLOCK));
        }
        // SAFETY: `handle` is an open thread handle.
        let r = unsafe { WaitForSingleObject(self.handle, INFINITE) };
        if r == WAIT_FAILED {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Returns the thread's handle, which stays owned by `self`.
    pub(crate) const fn as_raw(&self) -> HANDLE {
        self.handle
    }

    /// Returns the thread's handle without closing it; the caller must.
    pub(crate) fn into_raw(self) -> HANDLE {
        core::mem::ManuallyDrop::new(self).handle
    }
}

impl Drop for Thread {
    fn drop(&mut self) {
        // SAFETY: `handle` is an open thread handle owned by `self`.
        let r = unsafe { CloseHandle(self.handle) };
        // Closing a valid handle cannot fail.
        debug_assert_ne!(r, 0);
    }
}

unsafe extern "system" fn thread_start(arg: *mut c_void) -> u32 {
    // `Thread::new` always passes a non-null pointer; checking it here keeps
    // the invariant local for one branch per thread.
    let Some(start) = NonNull::new(arg.cast::<Start>()) else {
        super::os::abort();
    };
    // SAFETY: `start` points to a live `Start` whose owner guarantees that
    // calling `main` with it here is sound. A panic that unwinds out of
    // `main` aborts the process at this `extern "system"` boundary.
    unsafe {
        let main = start.as_ref().main;
        main(start);
    }
    0
}

/// The id of the thread that ran the C runtime's initializers, or 0.
static MAIN_THREAD: AtomicU32 = AtomicU32::new(0);

/// Records the main thread: the C runtime calls the `.CRT$XCU` functions on
/// it before `main` (in a DLL, on the loading thread), and Windows has no
/// call that names it later.
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static RECORD_MAIN_THREAD: extern "C" fn() = record_main_thread;

extern "C" fn record_main_thread() {
    // SAFETY: `GetCurrentThreadId` has no preconditions. Threads started
    // later observe the store through their creation.
    MAIN_THREAD.store(unsafe { GetCurrentThreadId() }, Relaxed);
}

/// Returns whether the calling thread is the process's main thread.
pub(crate) fn is_main() -> bool {
    // SAFETY: `GetCurrentThreadId` has no preconditions.
    let id = unsafe { GetCurrentThreadId() };
    id == MAIN_THREAD.load(Relaxed)
}

/// Names the calling thread for debuggers, cut at its first NUL and after at
/// most 255 UTF-16 units, keeping whole characters.
pub(crate) fn set_name(name: &str) {
    /// UTF-16 units kept, including the terminating NUL.
    const MAX_UNITS: usize = 256;
    let mut buf = [0u16; MAX_UNITS];
    let mut len = 0usize;
    // A NUL character, a zero unit, ends the name.
    let units = name.encode_utf16().take_while(|&unit| unit != 0);
    for (dst, unit) in buf[..MAX_UNITS - 1].iter_mut().zip(units) {
        *dst = unit;
        len += 1;
    }
    // A cut between the two halves of a surrogate pair leaves a high
    // surrogate last, which a complete string never ends with; drop it.
    if let Some(last) = len.checked_sub(1).and_then(|i| buf.get_mut(i)) {
        if (0xD800..0xDC00).contains(last) {
            *last = 0;
        }
    }
    // SAFETY: `buf` is NUL-terminated; the pseudo handle is this thread.
    let hr = unsafe { SetThreadDescription(GetCurrentThread(), buf.as_ptr()) };
    // Naming is best effort; it only fails without memory.
    debug_assert!(hr >= 0);
}

/// Blocks the calling thread for at least `dur`.
pub(crate) fn sleep(dur: Duration) {
    if dur.is_zero() {
        // `Sleep(0)` yields the rest of the time slice, as std's does.
        // SAFETY: `Sleep` has no preconditions.
        unsafe { Sleep(0) };
    } else if !sleep_precise(dur) {
        sleep_coarse(dur);
    }
}

/// Sleeps on a high-resolution waitable timer, returning `false` if the
/// timer could not be used.
fn sleep_precise(dur: Duration) -> bool {
    // Waitable timers count in 100 ns units, relative when negative. Round
    // up so the sleep is never short; more than 29,000 years does not fit
    // and takes the coarse path.
    let Some(units) = dur.as_secs().checked_mul(10_000_000).and_then(|units| {
        units.checked_add(u64::from(dur.subsec_nanos().div_ceil(100)))
    }) else {
        return false;
    };
    let Ok(units) = i64::try_from(units) else {
        return false;
    };
    // SAFETY: null attributes and name are allowed; flags and access are valid.
    let timer = unsafe {
        CreateWaitableTimerExW(
            ptr::null(),
            ptr::null(),
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
            TIMER_ALL_ACCESS,
        )
    };
    if timer.is_null() {
        return false;
    }
    let due = -units;
    // SAFETY: `timer` is an open timer handle and `due` outlives the call;
    // there is no completion routine.
    let set = unsafe {
        SetWaitableTimer(timer, &raw const due, 0, None, ptr::null(), 0)
    };
    // SAFETY: `timer` is an open timer handle.
    let waited = set != 0
        && unsafe { WaitForSingleObject(timer, INFINITE) } != WAIT_FAILED;
    // SAFETY: `timer` is an open handle owned here.
    let closed = unsafe { CloseHandle(timer) };
    debug_assert_ne!(closed, 0);
    waited
}

/// Sleeps with `Sleep`, which counts in milliseconds and may wake up to a
/// clock tick early.
fn sleep_coarse(dur: Duration) {
    let Some(deadline) = time::monotonic().checked_add(dur) else {
        // Past the clock's range, hundreds of billions of years away.
        // SAFETY: `Sleep` has no preconditions.
        unsafe { Sleep(INFINITE) };
        return;
    };
    // Each round sleeps for the time left until the deadline, so the loop
    // ends once the clock has passed it.
    loop {
        let left = deadline.saturating_sub(time::monotonic());
        if left.is_zero() {
            return;
        }
        // Rounded up so a round never ends early, and below `INFINITE`.
        let ms = timeout_ms(left, INFINITE - 1);
        // SAFETY: `Sleep` has no preconditions.
        unsafe { Sleep(ms) };
    }
}

/// Offers the rest of the calling thread's time slice to other threads.
pub(crate) fn yield_now() {
    // SAFETY: `SwitchToThread` has no preconditions. It returns 0 when no
    // other thread was ready, which is not an error.
    unsafe { SwitchToThread() };
}

/// Returns the number of logical processors in the process's group.
pub(crate) fn available_parallelism() -> io::Result<NonZero<usize>> {
    let mut info = SYSTEM_INFO::default();
    // SAFETY: `info` is valid for writes, and `GetSystemInfo` fills it.
    unsafe { GetSystemInfo(&raw mut info) };
    usize::try_from(info.dwNumberOfProcessors)
        .ok()
        .and_then(NonZero::new)
        .ok_or_else(unknown_parallelism)
}

#[cold]
const fn invalid_stack_size() -> io::Error {
    io::const_error!(io::ErrorKind::InvalidInput, "invalid stack size")
}

#[cold]
const fn unknown_parallelism() -> io::Error {
    io::const_error!(
        io::ErrorKind::NotFound,
        "the number of hardware threads is not known for the target platform"
    )
}
