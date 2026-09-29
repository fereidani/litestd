//! Clocks on top of `clock_gettime`.

use core::{mem::MaybeUninit, time::Duration};

/// The largest valid `tv_nsec`. Clamping to it lets the compiler drop the
/// carry path of `Duration::new`, which the kernel never needs.
const MAX_NANOS: u32 = 999_999_999;

/// Reads `clock` as seconds and nanoseconds.
fn read(clock: libc::clockid_t) -> (i64, u32) {
    let mut ts = MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: `ts` is valid for writes of a `timespec`.
    let r = unsafe { libc::clock_gettime(clock, ts.as_mut_ptr()) };
    if r != 0 {
        return read_wide(clock);
    }
    // SAFETY: `clock_gettime` returned 0, so it initialized `ts`.
    let ts = unsafe { ts.assume_init() };
    // `tv_sec` is 32 bits wide on some 32-bit targets.
    #[allow(clippy::useless_conversion)]
    let secs = i64::from(ts.tv_sec);
    // The kernel keeps `tv_nsec` in `0..1_000_000_000`, where the cast is
    // exact; the clamp alone guards the rest, with fewer instructions than a
    // checked conversion.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let nanos = (ts.tv_nsec as u32).min(MAX_NANOS);
    (secs, nanos)
}

/// Reads `clock` after `clock_gettime` failed. For a valid clock it fails
/// only with `EOVERFLOW`, where `time_t` has 32 bits and the time needs
/// more: the wall clock from 2038 on, or a clock that a time namespace moved
/// ahead. The 64-bit system call reads it then; without one, or should it
/// fail too, this reports the clock's origin.
#[cold]
fn read_wide(clock: libc::clockid_t) -> (i64, u32) {
    time64::clock_gettime(clock).unwrap_or((0, 0))
}

/// The 64-bit time system calls of 32-bit x86 and Arm, from Linux 5.1. The
/// libc crate lacks their numbers there.
#[cfg(all(
    any(target_os = "linux", target_os = "android"),
    any(target_arch = "x86", target_arch = "arm")
))]
pub(crate) mod time64 {
    const SYS_CLOCK_GETTIME64: libc::c_long = 403;

    /// A `__kernel_timespec`, which these system calls read and write.
    #[repr(C)]
    pub(crate) struct Timespec64 {
        pub(crate) tv_sec: i64,
        pub(crate) tv_nsec: i64,
    }

    /// Reads `clock` with `clock_gettime64`.
    pub(crate) fn clock_gettime(clock: libc::clockid_t) -> Option<(i64, u32)> {
        let mut ts = Timespec64 {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `ts` is valid for writes of a `__kernel_timespec`.
        let r =
            unsafe { libc::syscall(SYS_CLOCK_GETTIME64, clock, &raw mut ts) };
        let nanos = u32::try_from(ts.tv_nsec).unwrap_or(0);
        (r == 0).then_some((ts.tv_sec, nanos.min(super::MAX_NANOS)))
    }
}

/// Where `time_t` has 64 bits, or no 64-bit call is known to fall back on.
#[cfg(not(all(
    any(target_os = "linux", target_os = "android"),
    any(target_arch = "x86", target_arch = "arm")
)))]
mod time64 {
    pub(super) const fn clock_gettime(
        _: libc::clockid_t,
    ) -> Option<(i64, u32)> {
        None
    }
}

/// The clock behind `Instant`, as in std.
#[cfg(not(any(target_vendor = "apple", target_os = "wasi")))]
const MONOTONIC: libc::clockid_t = libc::CLOCK_MONOTONIC;
/// The clock behind `Instant`, as in std; wasi-libc's clocks are statics.
#[cfg(target_os = "wasi")]
use libc::CLOCK_MONOTONIC as MONOTONIC;
/// The clock behind `Instant`, as in std: `mach_absolute_time` in
/// nanoseconds, which stops while the system sleeps and which the kernel's
/// timeouts run on.
#[cfg(target_vendor = "apple")]
const MONOTONIC: libc::clockid_t = libc::CLOCK_UPTIME_RAW;

/// Reads the monotonic clock behind `Instant`.
pub(crate) fn monotonic() -> Duration {
    let (secs, nanos) = read(MONOTONIC);
    // The clock counts from boot and is never negative.
    Duration::new(u64::try_from(secs).unwrap_or(0), nanos)
}

/// Reads the wall clock as seconds and nanoseconds since the Unix epoch.
#[cfg(feature = "time")]
pub(crate) fn realtime() -> (i64, u32) {
    read(libc::CLOCK_REALTIME)
}

/// Sleeps until the absolute `deadline` of `monotonic`, for a module without
/// threads, where nothing else could end a wait sooner. Each sleep is
/// relative: not every WASI runtime honors an absolute one.
#[cfg(all(
    target_os = "wasi",
    any(
        feature = "thread",
        all(feature = "sync", not(target_feature = "atomics"))
    )
))]
pub(crate) fn sleep_until(deadline: Duration) {
    // Each pass sleeps the time left. A runtime whose timers are coarser
    // than the request wakes early, and the next pass sleeps the rest; the
    // clock moves on, so the loop ends once it passes `deadline`.
    while let Some(left) = deadline
        .checked_sub(monotonic())
        .filter(|left| !left.is_zero())
    {
        let ts = libc::timespec {
            tv_sec: i64::try_from(left.as_secs()).unwrap_or(i64::MAX),
            // Below one billion, so it fits the 32-bit `c_long`.
            tv_nsec: libc::c_long::try_from(left.subsec_nanos()).unwrap_or(0),
        };
        // SAFETY: `ts` is a valid `timespec`, and no time left is asked for.
        let r = unsafe {
            libc::clock_nanosleep(
                MONOTONIC,
                0,
                &raw const ts,
                core::ptr::null_mut(),
            )
        };
        // WASI has no signals to interrupt a sleep, and the request is valid.
        debug_assert_eq!(r, 0);
    }
}
