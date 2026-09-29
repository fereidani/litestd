//! Futex wait and wake on top of FreeBSD's `_umtx_op`.

#[cfg(any(feature = "sync", feature = "thread"))]
use core::time::Duration;
use core::{ffi::c_void, ptr, sync::atomic::AtomicU32};

use super::os;

/// The address the calls take; waits only read the word, atomically.
const fn addr(futex: &AtomicU32) -> *mut c_void {
    futex.as_ptr().cast()
}

/// Blocks in `UMTX_OP_WAIT_UINT_PRIVATE` while `futex` holds `expected`,
/// until the deadline in `time` unless it is null. Returns `false` if the
/// deadline passed.
///
/// # Safety
///
/// `time` must be null or point to a valid `_umtx_time`.
unsafe fn wait_op(
    futex: &AtomicU32,
    expected: u32,
    time: *mut libc::_umtx_time,
) -> bool {
    // The kernel takes the size of the timeout in place of a pointer.
    let size = if time.is_null() {
        0
    } else {
        size_of::<libc::_umtx_time>()
    };
    // Repeats only when a signal handler interrupted the wait (`EINTR`); the
    // deadline is absolute, so it stays. Signals are finite, so this ends.
    loop {
        // SAFETY: `futex` is a live, aligned `u32` the kernel reads
        // atomically, and the caller vouches for `time`.
        let r = unsafe {
            libc::_umtx_op(
                addr(futex),
                libc::UMTX_OP_WAIT_UINT_PRIVATE,
                expected.into(),
                ptr::without_provenance_mut(size),
                time.cast(),
            )
        };
        if r == 0 {
            return true;
        }
        match os::errno() {
            libc::EINTR => {}
            libc::ETIMEDOUT => return false,
            _ => return true,
        }
    }
}

/// Blocks while `futex` holds `expected`; the wait may end spuriously.
pub(crate) fn wait(futex: &AtomicU32, expected: u32) {
    // SAFETY: a null timeout waits without one.
    unsafe { wait_op(futex, expected, ptr::null_mut()) };
}

/// Blocks while `futex` holds `expected`, until the absolute `deadline` of
/// `time::monotonic`, `CLOCK_MONOTONIC`. Returns `false` only if the
/// deadline passed; the wait may also end spuriously.
#[cfg(any(feature = "sync", feature = "thread"))]
pub(crate) fn wait_until(
    futex: &AtomicU32,
    expected: u32,
    deadline: Duration,
) -> bool {
    // One that `time_t` cannot hold lies billions of years away.
    let Ok(secs) = libc::time_t::try_from(deadline.as_secs()) else {
        wait(futex, expected);
        return true;
    };
    let mut time = libc::_umtx_time {
        _timeout: libc::timespec {
            tv_sec: secs,
            tv_nsec: deadline.subsec_nanos().into(),
        },
        _flags: libc::UMTX_ABSTIME,
        #[allow(clippy::cast_sign_loss, reason = "a valid clock id")]
        _clockid: libc::CLOCK_MONOTONIC as u32,
    };
    // SAFETY: `time` is a valid `_umtx_time`, live for the call.
    unsafe { wait_op(futex, expected, &raw mut time) }
}

/// Wakes up to `count` threads blocked on `futex`.
fn wake_op(futex: &AtomicU32, count: libc::c_ulong) {
    // SAFETY: the address only identifies the waiters.
    let r = unsafe {
        libc::_umtx_op(
            addr(futex),
            libc::UMTX_OP_WAKE_PRIVATE,
            count,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    // A private wake of a valid address cannot fail.
    debug_assert_eq!(r, 0);
}

/// Wakes one thread blocked on `futex`. Returns `false`, as FreeBSD does
/// not report whether a thread was woken.
pub(crate) fn wake(futex: &AtomicU32) -> bool {
    wake_op(futex, 1);
    false
}

/// Wakes every thread blocked on `futex`, for the locks of `sync` and
/// `env`.
#[cfg(any(feature = "sync", feature = "env"))]
pub(crate) fn wake_all(futex: &AtomicU32) {
    #[allow(clippy::cast_sign_loss, reason = "a positive count")]
    wake_op(futex, libc::c_int::MAX as libc::c_ulong);
}
