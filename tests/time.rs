//! `litestd::time` against std's documented behavior.

// wasm32-unknown-unknown has no clock and no times before the epoch:
// `tests/unsupported.rs` compares its times with std's.
#![cfg(all(feature = "time", not(target_os = "unknown")))]

use core::{
    cmp::Ordering,
    hash::{Hash, Hasher},
    mem::size_of,
    panic::{RefUnwindSafe, UnwindSafe},
};
use std::collections::hash_map::DefaultHasher;

use litestd::time::{
    Duration, Instant, SystemTime, SystemTimeError, TryFromFloatSecsError,
    UNIX_EPOCH,
};

fn hash_of<T: Hash>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[test]
fn instant_is_monotonic() {
    let mut previous = Instant::now();
    for _ in 0..1000 {
        let now = Instant::now();
        assert!(now >= previous);
        previous = now;
    }
}

#[test]
fn instant_tracks_std() {
    let (lite_start, std_start) = (Instant::now(), std::time::Instant::now());
    // Some WASI runtimes wake a sleep early, so it repeats until std's clock
    // moved on; litestd's started first, so it moved as far.
    while std_start.elapsed() < Duration::from_millis(20) {
        std::thread::sleep(Duration::from_millis(5));
    }
    let lite = lite_start.elapsed();
    let std = std_start.elapsed();
    assert!(lite >= Duration::from_millis(20));
    // Both read the same clock, so the readings agree closely.
    assert!(
        lite.abs_diff(std) < Duration::from_millis(15),
        "{lite:?} {std:?}"
    );
}

#[test]
fn instant_arithmetic() {
    let now = Instant::now();
    let later = now + Duration::from_secs(1);
    assert_eq!(later - now, Duration::from_secs(1));
    assert_eq!(later.duration_since(now), Duration::from_secs(1));
    assert_eq!(
        later.checked_duration_since(now),
        Some(Duration::from_secs(1))
    );
    assert_eq!(later - Duration::from_secs(1), now);

    // Earlier instants saturate, like std.
    assert_eq!(now.duration_since(later), Duration::ZERO);
    assert_eq!(now.saturating_duration_since(later), Duration::ZERO);
    assert_eq!(now.checked_duration_since(later), None);
    assert_eq!(now - later, Duration::ZERO);

    let mut t = now;
    t += Duration::from_millis(1500);
    t -= Duration::from_millis(500);
    assert_eq!(t, later);

    // Sub-second carries and borrows.
    let a = now + Duration::new(0, 999_999_999);
    let b = a + Duration::new(0, 2);
    assert_eq!(b - a, Duration::new(0, 2));
    assert_eq!(b - Duration::new(1, 1), now);
}

#[test]
fn instant_reaches_far_before_its_origin() {
    // std's Unix `Instant` holds signed seconds; litestd matches that range
    // on every platform.
    let now = Instant::now();
    let long_ago = now.checked_sub(Duration::from_secs(i64::MAX as u64 / 2));
    assert!(long_ago.is_some());
    let long_ago = long_ago.unwrap();
    assert!(long_ago < now);
    assert_eq!(now - long_ago, Duration::from_secs(i64::MAX as u64 / 2));
}

/// std declares `checked_add` and `checked_sub` without `#[must_use]`, so
/// code that ignores their result compiles without warnings against either.
#[test]
#[deny(unused_must_use)]
fn checked_arithmetic_is_not_must_use() {
    let now = Instant::now();
    now.checked_add(Duration::ZERO);
    now.checked_sub(Duration::ZERO);
    UNIX_EPOCH.checked_add(Duration::ZERO);
    UNIX_EPOCH.checked_sub(Duration::ZERO);
}

#[test]
fn instant_checked_overflow() {
    let now = Instant::now();
    assert_eq!(now.checked_add(Duration::MAX), None);
    assert_eq!(now.checked_sub(Duration::MAX), None);
    assert_eq!(now.checked_add(Duration::ZERO), Some(now));
}

#[test]
#[should_panic(expected = "overflow when adding duration to instant")]
fn instant_add_overflow_panics() {
    let _ = Instant::now() + Duration::MAX;
}

#[test]
#[should_panic(expected = "overflow when subtracting duration from instant")]
fn instant_sub_overflow_panics() {
    let _ = Instant::now() - Duration::MAX;
}

#[test]
#[should_panic(expected = "overflow when adding duration to instant")]
fn instant_add_assign_overflow_panics() {
    let mut t = Instant::now();
    t += Duration::MAX;
}

#[test]
#[should_panic(expected = "overflow when subtracting duration from instant")]
fn instant_sub_assign_overflow_panics() {
    let mut t = Instant::now();
    t -= Duration::MAX;
}

#[test]
fn instant_traits() {
    let a = Instant::now();
    let b = a + Duration::from_nanos(1);
    let c = a;
    assert_eq!(a, c);
    assert_ne!(a, b);
    assert!(a < b);
    assert_eq!(a.cmp(&b), Ordering::Less);
    assert_eq!(hash_of(&a), hash_of(&c));
    // std prints a `timespec` on Unix and the `Duration` since the
    // counter's origin on Windows; litestd follows both.
    let debug = format!("{a:?}");
    if cfg!(windows) {
        assert!(debug.starts_with("Instant { t: "), "{debug}");
    } else {
        assert!(debug.starts_with("Instant { tv_sec: "), "{debug}");
        assert!(debug.contains("tv_nsec: "), "{debug}");
    }
}

#[test]
fn instant_debug_matches_std_on_unix() {
    // std's Unix `Instant` prints the `CLOCK_MONOTONIC` reading, and so
    // does litestd; the seconds therefore agree.
    if cfg!(unix) {
        let lite = format!("{:?}", Instant::now());
        let std = format!("{:?}", std::time::Instant::now());
        let secs = |s: &str| {
            let rest = &s[s.find("tv_sec: ").unwrap() + 8..];
            rest[..rest.find(',').unwrap()].parse::<i64>().unwrap()
        };
        assert!((secs(&lite) - secs(&std)).abs() <= 1, "{lite} {std}");
    }
}

/// A fixed wall-clock time, for tests that do not need the current one.
const SOME_TIME: Duration = Duration::new(1_700_000_000, 123_456_789);

#[test]
fn system_time_tracks_std() {
    let lite = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let std = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    assert!(
        lite.abs_diff(std) < Duration::from_secs(2),
        "{lite:?} {std:?}"
    );
    assert!(lite > Duration::from_secs(1_600_000_000));
}

#[test]
fn unix_epoch() {
    assert_eq!(SystemTime::UNIX_EPOCH, UNIX_EPOCH);
    assert_eq!(
        UNIX_EPOCH.duration_since(UNIX_EPOCH).unwrap(),
        Duration::ZERO
    );
    // Windows prints 100 ns intervals instead; see `time_debug`.
    #[cfg(unix)]
    assert_eq!(
        format!("{UNIX_EPOCH:?}"),
        "SystemTime { tv_sec: 0, tv_nsec: 0 }"
    );
}

#[test]
fn system_time_before_1970() {
    let before = UNIX_EPOCH - Duration::new(1, 500_000_000);
    assert!(before < UNIX_EPOCH);
    #[cfg(unix)]
    assert_eq!(
        format!("{before:?}"),
        "SystemTime { tv_sec: -2, tv_nsec: 500000000 }"
    );
    let err = before.duration_since(UNIX_EPOCH).unwrap_err();
    assert_eq!(err.duration(), Duration::new(1, 500_000_000));
    assert_eq!(
        UNIX_EPOCH.duration_since(before).unwrap(),
        Duration::new(1, 500_000_000)
    );
    assert_eq!(before + Duration::new(1, 500_000_000), UNIX_EPOCH);

    // The year 1 is representable, like with std on Unix.
    let year_one = UNIX_EPOCH - Duration::from_secs(62_135_596_800);
    assert_eq!(
        UNIX_EPOCH.duration_since(year_one).unwrap(),
        Duration::from_secs(62_135_596_800)
    );
}

#[test]
fn system_time_arithmetic() {
    let now = UNIX_EPOCH + SOME_TIME;
    let later = now + Duration::from_secs(10);
    assert_eq!(later.duration_since(now).unwrap(), Duration::from_secs(10));
    let err = now.duration_since(later).unwrap_err();
    assert_eq!(err.duration(), Duration::from_secs(10));
    assert_eq!(later.checked_sub(Duration::from_secs(10)), Some(now));
    assert_eq!(now.checked_add(Duration::MAX), None);
    assert_eq!(now.checked_sub(Duration::MAX), None);
    #[cfg(unix)]
    assert_eq!(
        format!("{now:?}"),
        "SystemTime { tv_sec: 1700000000, tv_nsec: 123456789 }"
    );

    let mut t = now;
    t += Duration::from_secs(3);
    t -= Duration::from_secs(1);
    assert_eq!(t, now + Duration::from_secs(2));
}

#[test]
fn system_time_elapsed() {
    let now = SystemTime::now();
    assert!(now.elapsed().is_ok());
    let future = SystemTime::now() + Duration::from_secs(3600);
    assert!(
        future.elapsed().unwrap_err().duration() > Duration::from_secs(3500)
    );
}

#[test]
#[should_panic(expected = "overflow when adding duration to `SystemTime`")]
fn system_time_add_overflow_panics() {
    let _ = UNIX_EPOCH + SOME_TIME + Duration::MAX;
}

#[test]
#[should_panic(
    expected = "overflow when subtracting duration from `SystemTime`"
)]
fn system_time_sub_overflow_panics() {
    let _ = UNIX_EPOCH + SOME_TIME - Duration::MAX;
}

#[test]
fn system_time_error() {
    let err: SystemTimeError = UNIX_EPOCH
        .duration_since(UNIX_EPOCH + Duration::from_millis(5))
        .unwrap_err();
    assert_eq!(err.to_string(), "second time provided was later than self");
    let copy = err.clone();
    assert_eq!(copy.duration(), err.duration());
    assert_eq!(format!("{err:?}"), "SystemTimeError(5ms)");
    let as_error: &dyn core::error::Error = &err;
    assert!(as_error.source().is_none());
}

#[test]
fn system_time_traits() {
    let a = UNIX_EPOCH + SOME_TIME;
    let b = a;
    assert_eq!(a, b);
    assert_eq!(hash_of(&a), hash_of(&b));
    assert!(a < a + Duration::from_nanos(1));
}

#[test]
fn auto_traits_match_std() {
    fn assert_send_sync<T: Send + Sync + Unpin>() {}
    fn assert_unwind_safe<T: UnwindSafe + RefUnwindSafe>() {}
    assert_send_sync::<Instant>();
    assert_send_sync::<SystemTime>();
    assert_send_sync::<SystemTimeError>();
    assert_unwind_safe::<Instant>();
    assert_unwind_safe::<SystemTime>();
    assert_unwind_safe::<SystemTimeError>();
}

#[test]
fn reexports() {
    let _: TryFromFloatSecsError =
        Duration::try_from_secs_f64(-1.0).unwrap_err();
    // Both are a plain `Duration`, whose niche `Option` reuses.
    assert_eq!(size_of::<Instant>(), size_of::<Duration>());
    assert_eq!(size_of::<SystemTime>(), size_of::<Duration>());
    assert_eq!(size_of::<Option<Instant>>(), size_of::<Instant>());
}
