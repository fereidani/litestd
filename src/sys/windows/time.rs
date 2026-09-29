//! Clocks on top of `QueryPerformanceCounter` and the precise system time.

use core::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use windows_sys::Win32::System::Performance::{
    QueryPerformanceCounter, QueryPerformanceFrequency,
};
#[cfg(feature = "time")]
use windows_sys::Win32::{
    Foundation::FILETIME,
    System::SystemInformation::GetSystemTimePreciseAsFileTime,
};

const NANOS_PER_SEC: u64 = 1_000_000_000;

/// 100-nanosecond intervals per second: the unit of `FILETIME`.
pub(crate) const INTERVALS_PER_SEC: i64 = 10_000_000;

/// Seconds from 1601-01-01, the origin of `FILETIME`, to the Unix epoch.
#[cfg(feature = "time")]
pub(crate) const SECS_TO_UNIX_EPOCH: i64 = 11_644_473_600;

/// `FILETIME` intervals between 1601-01-01 and the Unix epoch.
#[cfg(feature = "time")]
const INTERVALS_TO_UNIX_EPOCH: i64 = SECS_TO_UNIX_EPOCH * INTERVALS_PER_SEC;

/// The performance counter frequency on every current Windows version: one
/// tick per `FILETIME` interval.
const USUAL_FREQUENCY: u64 = INTERVALS_PER_SEC.unsigned_abs();

/// The performance counter frequency in ticks per second, or 0 until the
/// first read. The frequency is fixed at boot, so racing initializations
/// store the same value, and `Relaxed` suffices for a single location.
static FREQUENCY: AtomicU64 = AtomicU64::new(0);

fn frequency() -> u64 {
    match FREQUENCY.load(Ordering::Relaxed) {
        0 => frequency_init(),
        freq => freq,
    }
}

#[cold]
fn frequency_init() -> u64 {
    let mut freq = 0i64;
    // SAFETY: `freq` is writable; the call cannot fail since Windows XP.
    unsafe { QueryPerformanceFrequency(&raw mut freq) };
    // Guards the divisions below; the documented minimum is far above 1.
    let freq = u64::try_from(freq).unwrap_or(1).max(1);
    FREQUENCY.store(freq, Ordering::Relaxed);
    freq
}

/// Reads the monotonic clock behind `Instant`.
pub(crate) fn monotonic() -> Duration {
    let mut ticks = 0i64;
    // SAFETY: `ticks` is writable; the call cannot fail since Windows XP.
    unsafe { QueryPerformanceCounter(&raw mut ticks) };
    let ticks = u64::try_from(ticks).unwrap_or(0);
    let freq = frequency();
    let (secs, nanos) = if freq == USUAL_FREQUENCY {
        // Dividing by a constant compiles to multiplications instead of two
        // hardware divisions.
        (
            ticks / USUAL_FREQUENCY,
            (ticks % USUAL_FREQUENCY) * (NANOS_PER_SEC / USUAL_FREQUENCY),
        )
    } else {
        // Split into whole seconds and a remainder so that the scaling
        // cannot overflow: `rem < freq`, and every real frequency is far
        // below `u64::MAX / NANOS_PER_SEC`.
        (
            ticks / freq,
            (ticks % freq).saturating_mul(NANOS_PER_SEC) / freq,
        )
    };
    // `nanos < NANOS_PER_SEC` because the remainder is below the frequency;
    // the clamp lets the compiler drop the carry path of `Duration::new`.
    let nanos = u32::try_from(nanos).unwrap_or(0).min(999_999_999);
    Duration::new(secs, nanos)
}

/// Reads the wall clock as seconds and nanoseconds since the Unix epoch.
#[cfg(feature = "time")]
pub(crate) fn realtime() -> (i64, u32) {
    let mut ft = FILETIME::default();
    // SAFETY: `ft` is valid for writes, which is all the call does with it.
    unsafe { GetSystemTimePreciseAsFileTime(&raw mut ft) };
    let intervals =
        (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime);
    // A `FILETIME` below 2^63 covers the next 29,000 years.
    let since_epoch =
        i64::try_from(intervals).unwrap_or(i64::MAX) - INTERVALS_TO_UNIX_EPOCH;
    let secs = since_epoch.div_euclid(INTERVALS_PER_SEC);
    // `rem_euclid` is in `0..INTERVALS_PER_SEC`, so the nanoseconds fit.
    let nanos = u32::try_from(since_epoch.rem_euclid(INTERVALS_PER_SEC) * 100)
        .unwrap_or(0);
    (secs, nanos)
}
