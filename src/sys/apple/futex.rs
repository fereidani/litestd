//! Futex wait and wake on top of `os_sync_wait_on_address`, from macOS 14.4.

#[cfg(any(feature = "sync", feature = "thread"))]
use core::time::Duration;
use core::{ffi::c_void, sync::atomic::AtomicU32};

use super::os;
#[cfg(any(feature = "sync", feature = "thread"))]
use super::time;

/// The futex word's size, which every wait and wake on it must pass alike.
const SIZE: usize = size_of::<u32>();

/// The address the calls take: they only read the futex word, atomically.
const fn addr(futex: &AtomicU32) -> *mut c_void {
    futex.as_ptr().cast()
}

/// Blocks while `futex` holds `expected`; the wait may end spuriously.
pub(crate) fn wait(futex: &AtomicU32, expected: u32) {
    // SAFETY: `futex` is a live, aligned `u32` for the whole call, and the
    // size and flags are valid.
    let r = unsafe {
        libc::os_sync_wait_on_address(
            addr(futex),
            expected.into(),
            SIZE,
            libc::OS_SYNC_WAIT_ON_ADDRESS_NONE,
        )
    };
    // A signal (`EINTR`), a page the kernel could not read at once
    // (`EFAULT`) or low memory (`ENOMEM`) end the wait early, as a spurious
    // wakeup does: the caller checks the futex and waits again.
    debug_assert!(
        r >= 0
            || matches!(os::errno(), libc::EINTR | libc::EFAULT | libc::ENOMEM)
    );
}

/// Blocks while `futex` holds `expected`, until the absolute `deadline` of
/// `time::monotonic`. Returns `false` only if the deadline passed; the wait
/// may also end spuriously.
///
/// The wait takes the time left in nanoseconds, which the kernel measures
/// on the same clock in its own units: that keeps it right whatever the
/// timebase is, under Rosetta too, for one clock read per wait.
#[cfg(any(feature = "sync", feature = "thread"))]
pub(crate) fn wait_until(
    futex: &AtomicU32,
    expected: u32,
    deadline: Duration,
) -> bool {
    // The kernel rounds the timeout down to whole ticks of its clock, so a
    // wait may time out a tick before the deadline: it then goes on for the
    // time left, and the deadline passes after a few rounds.
    loop {
        // A zero timeout is invalid, and the deadline has passed then too.
        let left = deadline.saturating_sub(time::monotonic());
        if left.is_zero() {
            return false;
        }
        // Beyond 2^64 nanoseconds, 584 years, the wait goes on without a
        // timeout.
        let Ok(timeout_ns) = u64::try_from(left.as_nanos()) else {
            wait(futex, expected);
            return true;
        };
        // SAFETY: as in `wait`; the clock is the one the timeout form
        // accepts.
        let r = unsafe {
            libc::os_sync_wait_on_address_with_timeout(
                addr(futex),
                expected.into(),
                SIZE,
                libc::OS_SYNC_WAIT_ON_ADDRESS_NONE,
                libc::OS_CLOCK_MACH_ABSOLUTE_TIME,
                timeout_ns,
            )
        };
        if r >= 0 || os::errno() != libc::ETIMEDOUT {
            return true;
        }
    }
}

/// Orders the caller's last store to the futex before the wake that
/// follows, as the kernel does by taking the lock that waits take before
/// they read the futex. Miri's wake returns before its fence when no thread
/// waits, so a thread that waits next could read an older value there.
#[cfg(miri)]
fn fence_for_miri() {
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}

/// Wakes one thread blocked on `futex`; returns whether it woke one.
pub(crate) fn wake(futex: &AtomicU32) -> bool {
    #[cfg(miri)]
    fence_for_miri();
    // SAFETY: the address only identifies the waiters, with the same size
    // and flags as theirs.
    let r = unsafe {
        libc::os_sync_wake_by_address_any(
            addr(futex),
            SIZE,
            libc::OS_SYNC_WAKE_BY_ADDRESS_NONE,
        )
    };
    // It fails with `ENOENT` when no thread waits.
    r == 0
}

/// Wakes every thread blocked on `futex`, for the locks of `sync` and
/// `env`.
#[cfg(any(feature = "sync", feature = "env"))]
pub(crate) fn wake_all(futex: &AtomicU32) {
    #[cfg(miri)]
    fence_for_miri();
    // SAFETY: as in `wake`.
    let r = unsafe {
        libc::os_sync_wake_by_address_all(
            addr(futex),
            SIZE,
            libc::OS_SYNC_WAKE_BY_ADDRESS_NONE,
        )
    };
    // It fails only with `ENOENT`, when no thread waits.
    debug_assert!(r == 0 || os::errno() == libc::ENOENT);
}
