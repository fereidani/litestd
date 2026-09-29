//! Futex wait and wake on top of DragonFly's `umtx_sleep` and
//! `umtx_wakeup`, whose timeouts are relative microseconds.

use core::sync::atomic::AtomicU32;
#[cfg(any(feature = "sync", feature = "thread"))]
use core::time::Duration;

use libc::c_int;

use super::os;
#[cfg(any(feature = "sync", feature = "thread"))]
use super::time;

/// The address the calls take; waits only read the word, atomically.
const fn addr(futex: &AtomicU32) -> *const c_int {
    futex.as_ptr().cast_const().cast()
}

/// Blocks while `futex` holds `expected`, for at most `timeout_us`
/// microseconds unless that is zero. Returns `false` if the timeout may have
/// elapsed: DragonFly reports a timeout as `EWOULDBLOCK`, and `ETIMEDOUT` is
/// taken for one too.
fn wait_op(futex: &AtomicU32, expected: u32, timeout_us: c_int) -> bool {
    // The kernel compares the word's bits, whatever its sign.
    #[allow(clippy::cast_possible_wrap, reason = "the value's bits")]
    let expected = expected as c_int;
    // SAFETY: `futex` is a live, aligned `u32` the kernel reads atomically.
    let r = unsafe { libc::umtx_sleep(addr(futex), expected, timeout_us) };
    // `EBUSY`: the futex no longer held `expected`; `EINTR`: a signal.
    r == 0 || !matches!(os::errno(), libc::EWOULDBLOCK | libc::ETIMEDOUT)
}

/// Blocks while `futex` holds `expected`; the wait may end spuriously.
pub(crate) fn wait(futex: &AtomicU32, expected: u32) {
    wait_op(futex, expected, 0);
}

/// Blocks while `futex` holds `expected`, until the absolute `deadline` of
/// `time::monotonic`. Returns `false` only if the deadline passed; the wait
/// may also end spuriously.
#[cfg(any(feature = "sync", feature = "thread"))]
pub(crate) fn wait_until(
    futex: &AtomicU32,
    expected: u32,
    deadline: Duration,
) -> bool {
    // A wait beyond the longest timeout the call takes, 35 minutes, ends
    // before the deadline and goes on for the time left, over a few rounds.
    loop {
        let left = deadline.saturating_sub(time::monotonic());
        if left.is_zero() {
            return false;
        }
        // Rounded up, so that the wait does not end early.
        let timeout_us = c_int::try_from(left.as_nanos().div_ceil(1000))
            .unwrap_or(c_int::MAX);
        if wait_op(futex, expected, timeout_us) {
            return true;
        }
    }
}

/// Wakes up to `count` threads blocked on `futex`.
fn wake_op(futex: &AtomicU32, count: c_int) {
    // SAFETY: the address only identifies the waiters.
    let r = unsafe { libc::umtx_wakeup(addr(futex), count) };
    // Waking a valid address cannot fail.
    debug_assert!(r >= 0);
}

/// Wakes one thread blocked on `futex`. Returns `false`, as DragonFly does
/// not report whether a thread was woken.
pub(crate) fn wake(futex: &AtomicU32) -> bool {
    wake_op(futex, 1);
    false
}

/// Wakes every thread blocked on `futex`, for the locks of `sync` and
/// `env`.
#[cfg(any(feature = "sync", feature = "env"))]
pub(crate) fn wake_all(futex: &AtomicU32) {
    wake_op(futex, c_int::MAX);
}
