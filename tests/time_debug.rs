//! `Debug` of `litestd::time::SystemTime` against std's on the same times:
//! the `timespec` fields on Unix, on Windows the number of 100 ns intervals
//! since 1601, and on wasm32-unknown-unknown, which has no times before the
//! epoch, the `Duration` since it. `fs::Metadata`, which prints its times,
//! then prints like std's as well.

#![cfg(feature = "time")]

#[cfg(all(feature = "fs", not(target_os = "unknown")))]
mod fs_util;

use core::time::Duration;

use litestd::time::{SystemTime, UNIX_EPOCH};

/// A time relative to the Unix epoch.
#[derive(Clone, Copy, Debug)]
enum Offset {
    After(Duration),
    Before(Duration),
}

use Offset::{After, Before};

impl Offset {
    fn lite(self) -> Option<SystemTime> {
        match self {
            After(d) => UNIX_EPOCH.checked_add(d),
            Before(d) => UNIX_EPOCH.checked_sub(d),
        }
    }

    fn real(self) -> Option<std::time::SystemTime> {
        match self {
            After(d) => std::time::UNIX_EPOCH.checked_add(d),
            Before(d) => std::time::UNIX_EPOCH.checked_sub(d),
        }
    }
}

/// `FILETIME` intervals from 1601-01-01 to the Unix epoch.
const INTERVALS_TO_UNIX_EPOCH: u64 = 116_444_736_000_000_000;

/// The largest `FILETIME` std's Windows `SystemTime` represents,
/// `i64::MAX` intervals, as an offset from the Unix epoch.
const WINDOWS_MAX: Duration = {
    let intervals = i64::MAX.unsigned_abs() - INTERVALS_TO_UNIX_EPOCH;
    #[allow(clippy::cast_possible_truncation, reason = "below 10^7")]
    let nanos = (intervals % 10_000_000) as u32 * 100;
    Duration::new(intervals / 10_000_000, nanos)
};

/// Whole 100 ns intervals, which std represents on every platform, from
/// 1601-01-01, the start of `FILETIME`, to its end in the year 30828.
const OFFSETS: [Offset; 11] = [
    After(Duration::ZERO),
    After(Duration::from_nanos(100)),
    After(Duration::from_secs(1)),
    After(Duration::new(1_700_000_000, 123_456_700)),
    After(Duration::from_secs(4_102_444_800)),
    After(WINDOWS_MAX),
    Before(Duration::from_nanos(100)),
    Before(Duration::new(1, 500_000_000)),
    Before(Duration::from_secs(31_536_000)),
    Before(Duration::new(11_644_473_599, 999_999_900)),
    Before(Duration::from_secs(11_644_473_600)),
];

#[test]
fn debug_matches_std() {
    for offset in OFFSETS {
        let (lite, real) = (offset.lite(), offset.real());
        assert_eq!(lite.is_some(), real.is_some(), "{offset:?}");
        let (Some(lite), Some(real)) = (lite, real) else {
            // Only wasm32-unknown-unknown, as std there, has no times before
            // the epoch.
            if cfg!(target_os = "unknown") {
                continue;
            }
            panic!("{offset:?} is out of range");
        };
        assert_eq!(format!("{lite:?}"), format!("{real:?}"), "{offset:?}");
        assert_eq!(format!("{lite:#?}"), format!("{real:#?}"), "{offset:?}");
    }
}

#[test]
#[cfg_attr(
    target_os = "unknown",
    ignore = "wasm32-unknown-unknown has no clock"
)]
fn debug_of_now_matches_std() {
    let real = std::time::SystemTime::now();
    let since = real.duration_since(std::time::UNIX_EPOCH).unwrap();
    let lite = UNIX_EPOCH + since;
    assert_eq!(format!("{lite:?}"), format!("{real:?}"));
    let now = format!("{:?}", SystemTime::now());
    let prefix = if cfg!(windows) {
        "SystemTime { intervals: "
    } else {
        "SystemTime { tv_sec: "
    };
    assert!(now.starts_with(prefix), "{now}");
}

#[test]
#[cfg_attr(
    target_os = "unknown",
    ignore = "wasm32-unknown-unknown has no times before the epoch"
)]
fn debug_of_times_std_cannot_represent() {
    // Beyond `FILETIME` on Windows, and parts of 100 ns anywhere: the
    // intervals round down, like the seconds of a `timespec`.
    let cases = [
        (
            After(WINDOWS_MAX + Duration::from_nanos(100)),
            "SystemTime { intervals: 9223372036854775808 }",
            "SystemTime { tv_sec: 910692730085, tv_nsec: 477580800 }",
        ),
        (
            Before(Duration::new(11_644_473_600, 100)),
            "SystemTime { intervals: -1 }",
            "SystemTime { tv_sec: -11644473601, tv_nsec: 999999900 }",
        ),
        (
            After(Duration::from_nanos(199)),
            "SystemTime { intervals: 116444736000000001 }",
            "SystemTime { tv_sec: 0, tv_nsec: 199 }",
        ),
        (
            Before(Duration::from_nanos(1)),
            "SystemTime { intervals: 116444735999999999 }",
            "SystemTime { tv_sec: -1, tv_nsec: 999999999 }",
        ),
        (
            // The latest time: `i64::MAX` seconds from the Unix epoch.
            After(Duration::new(i64::MAX.unsigned_abs(), 999_999_999)),
            "SystemTime { intervals: 92233720484992494079999999 }",
            "SystemTime { tv_sec: 9223372036854775807, tv_nsec: 999999999 }",
        ),
    ];
    for (offset, windows, unix) in cases {
        let time = offset.lite().unwrap();
        let expected = if cfg!(windows) { windows } else { unix };
        assert_eq!(format!("{time:?}"), expected, "{offset:?}");
    }
    // The earliest time: `i64::MIN` seconds from the Unix epoch.
    let min = UNIX_EPOCH - Duration::from_secs(1 << 63);
    assert_eq!(min.checked_sub(Duration::from_nanos(1)), None);
    let expected = if cfg!(windows) {
        "SystemTime { intervals: -92233720252103022080000000 }"
    } else {
        "SystemTime { tv_sec: -9223372036854775808, tv_nsec: 0 }"
    };
    assert_eq!(format!("{min:?}"), expected);
}

#[cfg(all(feature = "fs", not(target_os = "unknown")))]
#[test]
#[cfg_attr(miri, ignore = "Miri does not implement futimens")]
fn metadata_debug_matches_std() {
    let dir = fs_util::TempDir::new("time-debug");
    let path = dir.join("file");
    std::fs::write(&path, b"contents").unwrap();
    // Fixed times, so that nothing depends on the clock.
    let time =
        std::time::UNIX_EPOCH + Duration::new(1_600_000_000, 123_456_700);
    let times = std::fs::FileTimes::new()
        .set_accessed(time)
        .set_modified(time);
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    file.set_times(times).unwrap();
    drop(file);
    for path in [path, dir.path()] {
        let lite = litestd::fs::metadata(&path).unwrap();
        let real = std::fs::metadata(&path).unwrap();
        let (lite_time, real_time) =
            (lite.modified().unwrap(), real.modified().unwrap());
        assert_eq!(format!("{lite_time:?}"), format!("{real_time:?}"));
        let expected = fs_util::std_debug(&lite, &real);
        assert_eq!(format!("{lite:?}"), expected, "{path}");
    }
}

/// On Windows std shows an `Instant` as the `Duration` since the counter's
/// origin; litestd matches the format for every instant std can represent.
#[cfg(windows)]
#[test]
fn instant_debug_matches_std_format_on_windows() {
    let lite = format!("{:?}", litestd::time::Instant::now());
    let real = format!("{:?}", std::time::Instant::now());
    for s in [&lite, &real] {
        assert!(s.starts_with("Instant { t: ") && s.ends_with("s }"), "{s}");
    }
}
