//! Futex wait and wake on top of `WaitOnAddress`.

#[cfg(any(feature = "sync", feature = "thread"))]
use core::time::Duration;
use core::{ffi::c_void, ptr, sync::atomic::AtomicU32};

#[cfg(feature = "sync")]
use windows_sys::Win32::System::Threading::WakeByAddressAll;
use windows_sys::Win32::{
    Foundation::{ERROR_TIMEOUT, GetLastError},
    System::Threading::{INFINITE, WaitOnAddress, WakeByAddressSingle},
};

#[cfg(any(feature = "sync", feature = "thread"))]
use super::{os::timeout_ms, time};

/// Blocks while `futex` holds `expected`, for at most `ms` milliseconds
/// unless that is `INFINITE`. Returns `false` if the time ran out; wakeups
/// may be spurious.
fn wait_ms(futex: &AtomicU32, expected: u32, ms: u32) -> bool {
    // SAFETY: `futex` and `expected` are live, aligned 4-byte values for the
    // whole call, which only reads them.
    let woken = unsafe {
        WaitOnAddress(
            futex.as_ptr().cast::<c_void>(),
            ptr::from_ref(&expected).cast::<c_void>(),
            size_of::<u32>(),
            ms,
        )
    };
    // SAFETY: `GetLastError` has no preconditions.
    woken != 0 || unsafe { GetLastError() } != ERROR_TIMEOUT
}

/// Blocks while `futex` holds `expected`; the wait may end spuriously.
pub(crate) fn wait(futex: &AtomicU32, expected: u32) {
    let woken = wait_ms(futex, expected, INFINITE);
    // An infinite wait cannot time out.
    debug_assert!(woken);
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
    // A wait that runs out before the deadline goes on for the time left:
    // the timeout is capped just below `INFINITE`, over 49 days, and the
    // system may end a wait a clock tick early. Each round waits for about
    // the time left, so the deadline passes after a few.
    loop {
        let Some(left) = deadline.checked_sub(time::monotonic()) else {
            return false;
        };
        if wait_ms(futex, expected, timeout_ms(left, INFINITE - 1)) {
            return true;
        }
    }
}

/// Wakes one thread blocked on `futex`. Returns `false`, as Windows does
/// not report whether a thread was woken.
pub(crate) fn wake(futex: &AtomicU32) -> bool {
    // SAFETY: the address is used only as a key to find waiters.
    unsafe { WakeByAddressSingle(futex.as_ptr().cast::<c_void>()) };
    false
}

/// Wakes every thread blocked on `futex`. Only `sync` uses it.
#[cfg(feature = "sync")]
pub(crate) fn wake_all(futex: &AtomicU32) {
    // SAFETY: the address is used only as a key to find waiters.
    unsafe { WakeByAddressAll(futex.as_ptr().cast::<c_void>()) };
}
