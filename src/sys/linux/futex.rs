//! Futex wait and wake on top of `futex(2)`.

use core::{ptr, sync::atomic::AtomicU32};

use libc::{c_int, c_long};

#[cfg(any(feature = "sync", feature = "thread"))]
pub(crate) use self::deadline::wait_until;
use super::os;

/// Private futex operations: the futex is never shared between processes,
/// which lets the kernel skip the shared-mapping lookup.
const WAIT: c_int = libc::FUTEX_WAIT_BITSET | libc::FUTEX_PRIVATE_FLAG;
const WAKE: c_int = libc::FUTEX_WAKE | libc::FUTEX_PRIVATE_FLAG;

/// Matches every waker: `FUTEX_WAIT_BITSET` becomes a plain absolute wait.
const MATCH_ANY: u32 = u32::MAX;

/// Blocks in the wait system call `sys` while `futex` holds `expected`,
/// until the absolute `CLOCK_MONOTONIC` time at `deadline` unless it is
/// null. Returns the error of a wait that ended neither by a wakeup nor by
/// a changed value, retrying after signal handlers. `deadline` has the
/// type of the C prototype, as a variadic argument must, whatever layout
/// it points to.
///
/// # Safety
///
/// `deadline` must be null or point to the timeout layout that `sys` reads.
unsafe fn futex_wait(
    sys: c_long,
    futex: &AtomicU32,
    expected: u32,
    deadline: *const libc::timespec,
) -> Option<c_int> {
    // Repeats only when a signal handler interrupted the wait (`EINTR`);
    // every other outcome returns. Signals are finite, so the loop ends.
    loop {
        // SAFETY: `futex` is a live, aligned `u32` the kernel reads
        // atomically; the caller vouches for `deadline`. `FUTEX_WAIT_BITSET`
        // ignores `uaddr2`.
        let r = unsafe {
            libc::syscall(
                sys,
                futex.as_ptr(),
                WAIT,
                expected,
                deadline,
                ptr::null::<u32>(),
                MATCH_ANY,
            )
        };
        if r == 0 {
            return None;
        }
        match os::errno() {
            libc::EINTR => {}
            // The futex no longer held `expected`.
            libc::EAGAIN => return None,
            error => return Some(error),
        }
    }
}

/// Blocks while `futex` holds `expected`; the wait may end spuriously.
pub(crate) fn wait(futex: &AtomicU32, expected: u32) {
    // SAFETY: a null deadline waits without one.
    let error =
        unsafe { futex_wait(libc::SYS_futex, futex, expected, ptr::null()) };
    // Every kernel has `SYS_futex`, and the arguments are valid.
    debug_assert_eq!(error, None);
}

/// Waits with deadlines, which only `sync` and `thread` use.
#[cfg(any(feature = "sync", feature = "thread"))]
mod deadline {
    use core::{sync::atomic::AtomicU32, time::Duration};

    use libc::c_int;

    #[cfg(any(target_arch = "x86", target_arch = "arm"))]
    use super::super::time::time64::Timespec64;
    use super::{futex_wait, wait};

    /// Blocks while `futex` holds `expected`, until the absolute `deadline`
    /// of `time::monotonic`, `CLOCK_MONOTONIC`. Returns `false` only if the
    /// deadline passed; the wait may also end spuriously.
    pub(crate) fn wait_until(
        futex: &AtomicU32,
        expected: u32,
        deadline: Duration,
    ) -> bool {
        let error = KernelTimespec::new(deadline).map_or_else(
            || wait_wide(futex, expected, deadline),
            // SAFETY: `ts` is the layout `SYS_futex` reads, live for the
            // call.
            |ts| unsafe {
                let ts = (&raw const ts).cast();
                futex_wait(libc::SYS_futex, futex, expected, ts)
            },
        );
        error != Some(libc::ETIMEDOUT)
    }

    /// A field of the timeout that `SYS_futex` reads: a `__kernel_timespec`
    /// of two 64-bit fields on 64-bit targets and x32, and an
    /// `old_timespec32` of two C `long`s on the other 32-bit targets.
    /// `libc::timespec` matches neither everywhere.
    #[cfg(any(target_pointer_width = "64", target_arch = "x86_64"))]
    type KernelLong = i64;
    #[cfg(not(any(target_pointer_width = "64", target_arch = "x86_64")))]
    type KernelLong = libc::c_long;

    /// The timeout layout `SYS_futex` reads; see [`KernelLong`].
    #[repr(C)]
    struct KernelTimespec {
        tv_sec: KernelLong,
        tv_nsec: KernelLong,
    }

    impl KernelTimespec {
        /// Converts a `CLOCK_MONOTONIC` reading, or returns `None` if the
        /// seconds do not fit.
        fn new(t: Duration) -> Option<Self> {
            let secs = KernelLong::try_from(t.as_secs()).ok()?;
            // The field is 32 bits wide on some targets, where this
            // conversion is fallible; nanoseconds below one second always
            // fit.
            #[allow(clippy::unnecessary_fallible_conversions)]
            let nanos = KernelLong::try_from(t.subsec_nanos()).ok()?;
            Some(Self {
                tv_sec: secs,
                tv_nsec: nanos,
            })
        }
    }

    /// Waits for `deadline`, which does not fit the timeout of `SYS_futex`.
    /// On 32-bit targets that is from 2^31 seconds of the monotonic clock
    /// on, which a time namespace may move it past, and `futex_time64` takes
    /// it. Kernels without `futex_time64` predate time namespaces, so the
    /// deadline lies decades away there and the wait goes on without one; so
    /// does one that no system call can hold.
    #[cold]
    // Only x86 and Arm have a wider call that takes the deadline.
    #[cfg_attr(
        not(any(target_arch = "x86", target_arch = "arm")),
        allow(unused_variables)
    )]
    fn wait_wide(
        futex: &AtomicU32,
        expected: u32,
        deadline: Duration,
    ) -> Option<c_int> {
        #[cfg(any(target_arch = "x86", target_arch = "arm"))]
        if let Ok(tv_sec) = i64::try_from(deadline.as_secs()) {
            /// `futex_time64`, from Linux 5.1; the libc crate lacks it.
            const SYS_FUTEX_TIME64: libc::c_long = 422;
            let ts = Timespec64 {
                tv_sec,
                tv_nsec: deadline.subsec_nanos().into(),
            };
            let ts = (&raw const ts).cast();
            // SAFETY: `ts` is the `__kernel_timespec` that `futex_time64`
            // reads.
            let error =
                unsafe { futex_wait(SYS_FUTEX_TIME64, futex, expected, ts) };
            if error != Some(libc::ENOSYS) {
                return error;
            }
        }
        wait(futex, expected);
        None
    }
}

/// Wakes one thread blocked on `futex`; returns whether it woke one.
pub(crate) fn wake(futex: &AtomicU32) -> bool {
    // SAFETY: a private `FUTEX_WAKE` uses the address only as a key to find
    // waiters; `futex` is live for the call either way.
    let r = unsafe { libc::syscall(libc::SYS_futex, futex.as_ptr(), WAKE, 1) };
    r > 0
}

/// Wakes every thread blocked on `futex`, for the locks of `sync` and
/// `env`.
#[cfg(any(feature = "sync", feature = "env"))]
pub(crate) fn wake_all(futex: &AtomicU32) {
    // SAFETY: as in `wake`.
    let r = unsafe {
        libc::syscall(libc::SYS_futex, futex.as_ptr(), WAKE, c_int::MAX)
    };
    // `FUTEX_WAKE` on a valid private futex cannot fail.
    debug_assert!(r >= 0);
}
