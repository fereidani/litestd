//! The clocks of a module without an OS, which has none: reading one panics,
//! as in std.

use core::time::Duration;

/// Panics as std does without a clock.
#[cold]
#[track_caller]
#[allow(clippy::panic, reason = "std panics here")]
fn no_clock() -> ! {
    panic!("time not implemented on this platform")
}

pub(crate) fn monotonic() -> Duration {
    no_clock()
}

#[cfg(feature = "time")]
pub(crate) fn realtime() -> (i64, u32) {
    no_clock()
}

/// Panics as std does: there is nothing to sleep on. With threads the futex
/// waits with the instruction's own timeouts instead.
#[cfg(all(
    not(target_feature = "atomics"),
    any(feature = "sync", feature = "thread")
))]
#[cold]
#[track_caller]
#[allow(clippy::panic, reason = "std panics here")]
pub(crate) fn sleep_until(_deadline: Duration) {
    panic!("can't sleep")
}
