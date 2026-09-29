//! Temporal quantification.
//!
//! On Unix, `Instant` reads `CLOCK_MONOTONIC`, or on macOS, as in std,
//! `CLOCK_UPTIME_RAW`, which stops while the system sleeps, and `SystemTime`
//! reads `CLOCK_REALTIME`; on Windows, they read `QueryPerformanceCounter`
//! and `GetSystemTimePreciseAsFileTime`. Reading a clock never allocates.

// Not `const`, as in std.
#![allow(clippy::missing_const_for_fn)]

pub use core::time::{Duration, TryFromFloatSecsError};

#[cfg(feature = "time")]
pub use self::clocks::{Instant, SystemTime, SystemTimeError, UNIX_EPOCH};

/// The clocks, which need the OS.
#[cfg(feature = "time")]
mod clocks {
    use core::{
        error, fmt,
        ops::{Add, AddAssign, Sub, SubAssign},
        time::Duration,
    };

    use crate::sys;

    /// Maps signed clock seconds onto `Duration`'s unsigned seconds in order
    /// (`i64::MIN` to 0, 0 to `2^63`), so that times get the range of a
    /// signed second count and reuse `Duration`'s arithmetic. As in std,
    /// wasm32-unknown-unknown, which has no clock, has no times before the
    /// origin.
    const ORIGIN: u64 = if cfg!(target_os = "unknown") {
        0
    } else {
        1 << 63
    };

    /// Converts signed seconds to the offset scale.
    const fn offset_secs(secs: i64) -> u64 {
        // Flipping the top bit of the two's complement adds `2^63` mod `2^64`.
        #[allow(clippy::cast_sign_loss)]
        let bits = secs as u64;
        bits ^ ORIGIN
    }

    /// Converts offset seconds back to signed seconds.
    #[cfg(not(target_os = "unknown"))]
    const fn signed_secs(secs: u64) -> i64 {
        #[allow(clippy::cast_possible_wrap)]
        let secs = (secs ^ ORIGIN) as i64;
        secs
    }

    /// Formats an offset time the way std formats a Unix `timespec`.
    #[cfg(not(target_os = "unknown"))]
    fn debug_timespec(
        f: &mut fmt::Formatter<'_>,
        name: &str,
        t: Duration,
    ) -> fmt::Result {
        f.debug_struct(name)
            .field("tv_sec", &signed_secs(t.as_secs()))
            .field("tv_nsec", &t.subsec_nanos())
            .finish()
    }

    /// Implements the arithmetic of a time type over its `Duration` field:
    /// `checked_add`, `checked_sub` and the operators, which panic with the
    /// given messages when the result cannot be represented, as std
    /// documents. Each message is one cold function.
    macro_rules! duration_ops {
        ($t:ident, $add_overflow:literal, $sub_overflow:literal) => {
            impl $t {
                /// Returns `self + duration`, or `None` if it cannot be
                /// represented.
                #[allow(
                    clippy::must_use_candidate,
                    reason = "not `#[must_use]` in std"
                )]
                pub fn checked_add(&self, duration: Duration) -> Option<Self> {
                    self.0.checked_add(duration).map(Self)
                }

                /// Returns `self - duration`, or `None` if it cannot be
                /// represented.
                #[allow(
                    clippy::must_use_candidate,
                    reason = "not `#[must_use]` in std"
                )]
                pub fn checked_sub(&self, duration: Duration) -> Option<Self> {
                    self.0.checked_sub(duration).map(Self)
                }
            }

            impl Add<Duration> for $t {
                type Output = Self;

                /// # Panics
                ///
                /// Panics if the result cannot be represented.
                #[track_caller]
                fn add(self, dur: Duration) -> Self {
                    #[cold]
                    #[inline(never)]
                    #[track_caller]
                    #[allow(clippy::panic)]
                    fn overflow() -> ! {
                        panic!($add_overflow)
                    }
                    let Some(t) = self.checked_add(dur) else {
                        overflow()
                    };
                    t
                }
            }

            impl AddAssign<Duration> for $t {
                #[track_caller]
                fn add_assign(&mut self, dur: Duration) {
                    *self = *self + dur;
                }
            }

            impl Sub<Duration> for $t {
                type Output = Self;

                /// # Panics
                ///
                /// Panics if the result cannot be represented.
                #[track_caller]
                fn sub(self, dur: Duration) -> Self {
                    #[cold]
                    #[inline(never)]
                    #[track_caller]
                    #[allow(clippy::panic)]
                    fn overflow() -> ! {
                        panic!($sub_overflow)
                    }
                    let Some(t) = self.checked_sub(dur) else {
                        overflow()
                    };
                    t
                }
            }

            impl SubAssign<Duration> for $t {
                #[track_caller]
                fn sub_assign(&mut self, dur: Duration) {
                    *self = *self - dur;
                }
            }
        };
    }

    /// A measurement of a monotonically nondecreasing clock.
    ///
    /// It counts from an unspecified origin, the boot on Linux, macOS and
    /// Windows, and reaches about 292 billion years in either direction; on
    /// wasm32-unknown-unknown, as in std, only forward.
    #[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
    pub struct Instant(Duration);

    impl Instant {
        /// Returns an instant corresponding to "now".
        #[must_use]
        pub fn now() -> Self {
            let t = sys::time::monotonic();
            // The clock counts from boot, far below `2^63` seconds, so flipping
            // the top bit adds the offset without wrapping.
            Self(Duration::new(t.as_secs() ^ ORIGIN, t.subsec_nanos()))
        }

        /// Returns the time elapsed from `earlier`, or zero if it is later.
        #[must_use]
        pub fn duration_since(&self, earlier: Self) -> Duration {
            self.saturating_duration_since(earlier)
        }

        /// Returns the time elapsed from `earlier`, or `None` if it is later.
        #[must_use]
        pub fn checked_duration_since(
            &self,
            earlier: Self,
        ) -> Option<Duration> {
            self.0.checked_sub(earlier.0)
        }

        /// Returns the time elapsed from `earlier`, or zero if it is later.
        #[must_use]
        pub fn saturating_duration_since(&self, earlier: Self) -> Duration {
            self.0.saturating_sub(earlier.0)
        }

        /// Returns the time elapsed since this instant, or zero if it is
        /// later than now.
        #[must_use]
        pub fn elapsed(&self) -> Duration {
            Self::now().saturating_duration_since(*self)
        }
    }

    duration_ops!(
        Instant,
        "overflow when adding duration to instant",
        "overflow when subtracting duration from instant"
    );

    impl Sub<Self> for Instant {
        type Output = Duration;

        /// Returns the time elapsed from `other`, or zero if it is later.
        fn sub(self, other: Self) -> Duration {
            self.duration_since(other)
        }
    }

    #[cfg(not(target_os = "unknown"))]
    impl fmt::Debug for Instant {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            // std on Windows shows the `Duration` since the counter's origin;
            // an instant before it exists only in litestd.
            #[cfg(windows)]
            if let Ok(secs) = u64::try_from(signed_secs(self.0.as_secs())) {
                let t = Duration::new(secs, self.0.subsec_nanos());
                return f.debug_struct("Instant").field("t", &t).finish();
            }
            debug_timespec(f, "Instant", self.0)
        }
    }

    /// A measurement of the system clock, which, unlike [`Instant`], can go
    /// backwards.
    ///
    /// It reaches about 292 billion years around [`UNIX_EPOCH`] with
    /// nanosecond precision, and on wasm32-unknown-unknown, as in std, only
    /// after it. `Debug` shows std's form: the `timespec` on Unix and WASI,
    /// the 100 ns intervals since 1601 on Windows, and the `Duration` since
    /// the epoch on wasm32-unknown-unknown.
    #[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
    pub struct SystemTime(Duration);

    /// An anchor in time: "1970-01-01 00:00:00 UTC" on every system.
    pub const UNIX_EPOCH: SystemTime = SystemTime(Duration::from_secs(ORIGIN));

    impl SystemTime {
        /// An anchor in time: "1970-01-01 00:00:00 UTC" on every system.
        pub const UNIX_EPOCH: Self = UNIX_EPOCH;

        /// Returns the system time corresponding to "now".
        #[must_use]
        pub fn now() -> Self {
            let (secs, nanos) = sys::time::realtime();
            // The backend returns `nanos < 1_000_000_000`; the clamp lets the
            // compiler drop the carry path of `Duration::new`.
            Self(Duration::new(offset_secs(secs), nanos.min(999_999_999)))
        }

        /// Returns the amount of time elapsed from an earlier point in time.
        ///
        /// # Errors
        ///
        /// Returns [`SystemTimeError`] with the difference if `earlier` is
        /// later, as after a clock adjustment.
        pub fn duration_since(
            &self,
            earlier: Self,
        ) -> Result<Duration, SystemTimeError> {
            self.0.checked_sub(earlier.0).ok_or_else(|| {
                SystemTimeError(earlier.0.saturating_sub(self.0))
            })
        }

        /// Returns the time elapsed since this system time.
        ///
        /// # Errors
        ///
        /// Returns [`SystemTimeError`] with the difference if `self` is later
        /// than now.
        pub fn elapsed(&self) -> Result<Duration, SystemTimeError> {
            Self::now().duration_since(*self)
        }
    }

    duration_ops!(
        SystemTime,
        "overflow when adding duration to `SystemTime`",
        "overflow when subtracting duration from `SystemTime`"
    );

    // The file systems' times; wasm32-unknown-unknown has no file system.
    #[cfg(all(feature = "fs", not(target_os = "unknown")))]
    impl SystemTime {
        /// Creates a time from seconds and nanoseconds since the Unix epoch,
        /// or `None` if `nanos` is not below one billion.
        pub(crate) const fn from_unix(secs: i64, nanos: u32) -> Option<Self> {
            if nanos < 1_000_000_000 {
                Some(Self(Duration::new(offset_secs(secs), nanos)))
            } else {
                None
            }
        }
    }

    // `Debug` uses `to_unix` on Windows.
    #[cfg(any(all(feature = "fs", not(target_os = "unknown")), windows))]
    impl SystemTime {
        /// Returns the seconds and nanoseconds (below one billion) since the
        /// Unix epoch.
        pub(crate) const fn to_unix(self) -> (i64, u32) {
            (signed_secs(self.0.as_secs()), self.0.subsec_nanos())
        }
    }

    // As std on Unix: the `timespec` relative to the Unix epoch.
    #[cfg(not(any(windows, target_os = "unknown")))]
    impl fmt::Debug for SystemTime {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            debug_timespec(f, "SystemTime", self.0)
        }
    }

    // As std on Windows: the `FILETIME` count of 100 ns since 1601-01-01.
    #[cfg(windows)]
    impl fmt::Debug for SystemTime {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            /// `FILETIME` intervals per second.
            const INTERVALS_PER_SEC: i128 = 10_000_000;
            /// Seconds from 1601-01-01 to the Unix epoch.
            const SECS_TO_UNIX_EPOCH: i128 = 11_644_473_600;
            let (secs, nanos) = self.to_unix();
            // Rounds toward the past, as the seconds do. An `i128` holds every
            // time, even beyond `FILETIME`'s range, without overflow.
            let intervals = (i128::from(secs) + SECS_TO_UNIX_EPOCH)
                * INTERVALS_PER_SEC
                + i128::from(nanos / 100);
            f.debug_struct("SystemTime")
                .field("intervals", &intervals)
                .finish()
        }
    }

    // As std without a clock: the `Duration` since the origin.
    #[cfg(target_os = "unknown")]
    impl fmt::Debug for Instant {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_tuple("Instant").field(&self.0).finish()
        }
    }

    #[cfg(target_os = "unknown")]
    impl fmt::Debug for SystemTime {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_tuple("SystemTime").field(&self.0).finish()
        }
    }

    /// An error returned from `SystemTime::duration_since` and `elapsed`,
    /// holding how far the other time lies in the opposite direction.
    #[derive(Clone, Debug)]
    pub struct SystemTimeError(Duration);

    impl SystemTimeError {
        /// Returns how far forward the second system time was from the first.
        #[must_use]
        pub fn duration(&self) -> Duration {
            self.0
        }
    }

    impl fmt::Display for SystemTimeError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("second time provided was later than self")
        }
    }

    impl error::Error for SystemTimeError {}
}
