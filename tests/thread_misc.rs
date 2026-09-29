//! Sleeping, yielding, `available_parallelism` and `panicking`.

#![cfg(feature = "thread")]

use core::time::Duration;
use std::time::Instant;

use litestd::thread;

#[test]
#[cfg_attr(
    target_os = "unknown",
    ignore = "wasm32-unknown-unknown has no clock"
)]
fn sleep_never_returns_early() {
    for dur in [
        Duration::ZERO,
        Duration::from_nanos(1),
        Duration::from_micros(1500),
        Duration::from_millis(10),
        Duration::new(0, 25_000_001),
    ] {
        let start = Instant::now();
        thread::sleep(dur);
        let elapsed = start.elapsed();
        assert!(elapsed >= dur, "slept {elapsed:?} for {dur:?}");
    }
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn sleep_on_many_threads() {
    let dur = Duration::from_millis(5);
    let handles: Vec<_> = (0..4)
        .map(|_| {
            thread::spawn(move || {
                let start = Instant::now();
                thread::sleep(dur);
                start.elapsed()
            })
        })
        .collect();
    for handle in handles {
        assert!(handle.join().unwrap() >= dur);
    }
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn huge_sleeps_neither_overflow_nor_panic() {
    // Each thread would sleep for ages; it is left detached, and the
    // process exits without waiting for it. A panic would abort the test.
    for dur in [Duration::MAX, Duration::from_secs(u64::MAX / 2)] {
        let handle = thread::spawn(move || thread::sleep(dur));
        thread::sleep(Duration::from_millis(20));
        assert!(!handle.is_finished());
    }
}

#[test]
fn yield_now_returns() {
    for _ in 0..10 {
        thread::yield_now();
    }
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn available_parallelism_is_at_least_one() {
    let count = thread::available_parallelism().unwrap();
    assert!(count.get() >= 1);
    // The value does not depend on the calling thread.
    let other = thread::spawn(thread::available_parallelism).join().unwrap();
    assert_eq!(other.unwrap(), count);
}

/// std caps the count at the process's cgroup CPU quota, and so must
/// litestd.
#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn available_parallelism_matches_std() {
    let lite = thread::available_parallelism().unwrap();
    assert_eq!(lite, std::thread::available_parallelism().unwrap());
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn panicking_is_always_false() {
    assert!(!thread::panicking());
    assert!(!thread::spawn(thread::panicking).join().unwrap());
}
