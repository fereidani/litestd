//! Futex wait and wake on top of the `futex` of NetBSD 10, a system call
//! reached through `syscall`, and of OpenBSD. Both take Linux's operations,
//! with timeouts that are relative. Waits use `FUTEX_WAIT` and not
//! `FUTEX_WAIT_BITSET`, which NetBSD reads from the seventh argument, where
//! `syscall` passes it on the stack. NetBSD's kernel reads the timeout for
//! every operation, so each call passes all seven arguments.

#[cfg(any(feature = "sync", feature = "thread"))]
use core::time::Duration;
use core::{ptr, sync::atomic::AtomicU32};

use libc::c_int;

use super::os;
#[cfg(any(feature = "sync", feature = "thread"))]
use super::time;

/// `SYS___futex` from NetBSD's `sys/syscall.h`, which the libc crate lacks.
#[cfg(target_os = "netbsd")]
const SYS_FUTEX: c_int = 166;

/// Private futex operations: the futex is never shared between processes.
const WAIT: c_int = libc::FUTEX_WAIT | libc::FUTEX_PRIVATE_FLAG;
const WAKE: c_int = libc::FUTEX_WAKE | libc::FUTEX_PRIVATE_FLAG;

/// Runs `op` on `futex` with `val` and `timeout`, and without the second
/// address, which `FUTEX_WAIT` and `FUTEX_WAKE` ignore.
///
/// # Safety
///
/// `timeout` must be null or point to a valid `timespec`.
#[inline]
unsafe fn futex_op(
    futex: &AtomicU32,
    op: c_int,
    val: c_int,
    timeout: *const libc::timespec,
) -> c_int {
    #[cfg(target_os = "netbsd")]
    let uaddr2 = ptr::null_mut::<c_int>();
    // SAFETY: `futex` is a live, aligned `u32` that the kernel accesses
    // atomically, and the caller vouches for `timeout`.
    #[cfg(target_os = "netbsd")]
    let r = unsafe {
        libc::syscall(SYS_FUTEX, futex.as_ptr(), op, val, timeout, uaddr2, 0, 0)
    };
    // SAFETY: as above.
    #[cfg(target_os = "openbsd")]
    let r = unsafe {
        libc::futex(futex.as_ptr(), op, val, timeout, ptr::null_mut())
    };
    r
}

/// Blocks while `futex` holds `expected`, for at most the relative
/// `timeout` unless it is null. Returns `false` if the timeout elapsed; a
/// signal (`EINTR`, and `ECANCELED` on OpenBSD) or a futex that no longer
/// holds `expected` (`EAGAIN`) ends the wait early, as a spurious wakeup.
///
/// # Safety
///
/// `timeout` must be null or point to a valid `timespec`.
unsafe fn wait_op(
    futex: &AtomicU32,
    expected: u32,
    timeout: *const libc::timespec,
) -> bool {
    // The kernel compares the word's bits, whatever its sign.
    #[allow(clippy::cast_possible_wrap, reason = "the value's bits")]
    let expected = expected as c_int;
    // SAFETY: the caller vouches for `timeout`.
    let r = unsafe { futex_op(futex, WAIT, expected, timeout) };
    r == 0 || os::errno() != libc::ETIMEDOUT
}

/// Blocks while `futex` holds `expected`; the wait may end spuriously.
pub(crate) fn wait(futex: &AtomicU32, expected: u32) {
    // SAFETY: a null timeout waits without one.
    unsafe { wait_op(futex, expected, ptr::null()) };
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
    // A timeout that the kernel rounded to its clock may end before the
    // deadline: the wait then goes on for the time left, and the deadline
    // passes after a few rounds.
    loop {
        let left = deadline.saturating_sub(time::monotonic());
        if left.is_zero() {
            return false;
        }
        // One that `time_t` cannot hold lies billions of years away.
        let Ok(secs) = libc::time_t::try_from(left.as_secs()) else {
            wait(futex, expected);
            return true;
        };
        let ts = libc::timespec {
            tv_sec: secs,
            tv_nsec: left.subsec_nanos().into(),
        };
        // SAFETY: `ts` is a valid `timespec`, live for the call.
        if unsafe { wait_op(futex, expected, &raw const ts) } {
            return true;
        }
    }
}

/// Wakes up to `count` threads blocked on `futex`, returning how many.
fn wake_op(futex: &AtomicU32, count: c_int) -> c_int {
    // SAFETY: the timeout is null.
    let r = unsafe { futex_op(futex, WAKE, count, ptr::null()) };
    // A private wake of a valid address cannot fail.
    debug_assert!(r >= 0);
    r
}

/// Wakes one thread blocked on `futex`; returns whether it woke one.
pub(crate) fn wake(futex: &AtomicU32) -> bool {
    wake_op(futex, 1) > 0
}

/// Wakes every thread blocked on `futex`, for the locks of `sync` and
/// `env`.
#[cfg(any(feature = "sync", feature = "env"))]
pub(crate) fn wake_all(futex: &AtomicU32) {
    wake_op(futex, c_int::MAX);
}
