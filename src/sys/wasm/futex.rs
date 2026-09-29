//! The futex of a module. Without the `atomics` target feature one thread
//! runs: nothing else can change a futex while it waits, and nothing else
//! waits. With it, threads share the memory and wait with
//! `memory.atomic.wait32`, which a browser's main thread may not execute.

use core::sync::atomic::AtomicU32;
#[cfg(not(target_feature = "atomics"))]
use core::sync::atomic::Ordering::Relaxed;
#[cfg(any(feature = "sync", feature = "thread"))]
use core::time::Duration;
#[cfg(target_feature = "atomics")]
use core::{arch::wasm32, ptr};

#[cfg(not(target_feature = "atomics"))]
use super::os;

/// Blocks while `futex` holds `expected`. No other thread can change it, so
/// a wait that would block is a deadlock, such as a `Mutex` locked twice or
/// a `Condvar` waited on: it aborts the process, where std panics.
#[cfg(not(target_feature = "atomics"))]
pub(crate) fn wait(futex: &AtomicU32, expected: u32) {
    if futex.load(Relaxed) == expected {
        os::abort();
    }
}

/// Blocks while `futex` holds `expected`, until the absolute `deadline` of
/// `time::monotonic`. No other thread can change it, so a wait that would
/// block sleeps to the deadline and returns `false`.
#[cfg(all(
    not(target_feature = "atomics"),
    any(feature = "sync", feature = "thread")
))]
pub(crate) fn wait_until(
    futex: &AtomicU32,
    expected: u32,
    deadline: Duration,
) -> bool {
    if futex.load(Relaxed) != expected {
        return true;
    }
    super::time::sleep_until(deadline);
    false
}

/// Wakes a waiter, of which there are none.
#[cfg(not(target_feature = "atomics"))]
pub(crate) fn wake(_futex: &AtomicU32) -> bool {
    false
}

/// Wakes every waiter, of which there are none.
#[cfg(all(
    not(target_feature = "atomics"),
    any(feature = "sync", all(target_os = "wasi", feature = "env"))
))]
pub(crate) fn wake_all(_futex: &AtomicU32) {}

/// Waits on `futex` while it holds `expected`, for at most `timeout`
/// nanoseconds, or forever if it is negative. Returns 2 on a timeout.
#[cfg(target_feature = "atomics")]
fn wait32(futex: &AtomicU32, expected: u32, timeout: i64) -> i32 {
    // SAFETY: `futex` is a live, aligned 32-bit atomic in the shared memory,
    // which the instruction reads atomically.
    unsafe {
        wasm32::memory_atomic_wait32(
            ptr::from_ref(futex).cast_mut().cast(),
            // The instruction compares the bits.
            i32::from_ne_bytes(expected.to_ne_bytes()),
            timeout,
        )
    }
}

/// Blocks while `futex` holds `expected`; it may return spuriously.
#[cfg(target_feature = "atomics")]
pub(crate) fn wait(futex: &AtomicU32, expected: u32) {
    wait32(futex, expected, -1);
}

/// Blocks while `futex` holds `expected`, until the absolute `deadline` of
/// `time::monotonic`, which the instruction takes as the time left. A
/// timeout that ends early goes on for the time left.
#[cfg(all(
    target_feature = "atomics",
    any(feature = "sync", feature = "thread")
))]
pub(crate) fn wait_until(
    futex: &AtomicU32,
    expected: u32,
    deadline: Duration,
) -> bool {
    // Each round waits for about the time left, so the deadline passes
    // after a few.
    loop {
        let Some(left) = deadline.checked_sub(super::time::monotonic()) else {
            return false;
        };
        // Beyond `i64::MAX` nanoseconds, 292 years, it waits forever.
        let timeout = i64::try_from(left.as_nanos()).unwrap_or(-1);
        if wait32(futex, expected, timeout) != 2 {
            return true;
        }
    }
}

/// Blocks for at least `dur` on a futex that nothing wakes, as std sleeps
/// on wasm32-unknown-unknown, which has no clock.
#[cfg(all(
    target_os = "unknown",
    target_feature = "atomics",
    feature = "thread"
))]
pub(crate) fn sleep(dur: Duration) {
    let futex = AtomicU32::new(0);
    let mut left = dur.as_nanos();
    // Each pass waits for up to 292 years and only ends by timing out, as
    // nothing else knows the futex; the loop ends once `left` runs out.
    while left > 0 {
        let chunk = i64::try_from(left).unwrap_or(i64::MAX);
        wait32(&futex, 0, chunk);
        left -= u128::from(chunk.unsigned_abs());
    }
}

/// Wakes one waiter, returning whether there was one.
#[cfg(target_feature = "atomics")]
pub(crate) fn wake(futex: &AtomicU32) -> bool {
    // SAFETY: as in `wait32`; the instruction only counts and wakes waiters.
    let woken = unsafe {
        wasm32::memory_atomic_notify(ptr::from_ref(futex).cast_mut().cast(), 1)
    };
    woken > 0
}

/// Wakes every waiter.
#[cfg(all(
    target_feature = "atomics",
    any(feature = "sync", all(target_os = "wasi", feature = "env"))
))]
pub(crate) fn wake_all(futex: &AtomicU32) {
    // SAFETY: as in `wake`.
    unsafe {
        wasm32::memory_atomic_notify(
            ptr::from_ref(futex).cast_mut().cast(),
            u32::MAX,
        )
    };
}
