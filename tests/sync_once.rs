//! `Once`: a single execution under contention, waiting for it, and the
//! never-poisoned state.
#![cfg(feature = "sync")]

use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
    time::Duration,
};
use std::thread;

use litestd::sync::Once;

const THREADS: usize = if cfg!(miri) { 4 } else { 16 };

/// An unsynchronized slot. The initializer writes it and the other threads
/// read it after the `Once` completes, so Miri reports a data race if the
/// `Once` fails to order the write before the reads.
struct Slot(UnsafeCell<usize>);

// SAFETY: every access in these tests is ordered by the `Once` under test;
// a missing order is the data race the tests exist to catch.
unsafe impl Sync for Slot {}

impl Slot {
    const fn new() -> Self {
        Self(UnsafeCell::new(0))
    }

    /// # Safety
    ///
    /// No other access may run concurrently.
    unsafe fn set(&self, value: usize) {
        // SAFETY: guaranteed by the caller.
        unsafe { *self.0.get() = value };
    }

    /// # Safety
    ///
    /// No write may run concurrently.
    unsafe fn get(&self) -> usize {
        // SAFETY: guaranteed by the caller.
        unsafe { *self.0.get() }
    }
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn runs_once_under_contention() {
    let once = Once::new();
    let runs = AtomicUsize::new(0);
    let slot = Slot::new();
    thread::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                once.call_once(|| {
                    runs.fetch_add(1, Relaxed);
                    // SAFETY: only the initializer writes, before the `Once`
                    // completes.
                    unsafe { slot.set(42) };
                });
                assert!(once.is_completed());
                // SAFETY: `call_once` returned, so the write happened before.
                assert_eq!(unsafe { slot.get() }, 42);
            });
        }
    });
    assert_eq!(runs.load(Relaxed), 1);
}

#[test]
fn later_calls_do_nothing() {
    let once = Once::new();
    assert!(!once.is_completed());
    let runs = AtomicUsize::new(0);
    once.call_once(|| {
        runs.fetch_add(1, Relaxed);
    });
    once.call_once(|| {
        runs.fetch_add(1, Relaxed);
    });
    once.call_once_force(|_| {
        runs.fetch_add(1, Relaxed);
    });
    assert!(once.is_completed());
    assert_eq!(runs.load(Relaxed), 1);
}

#[test]
fn call_once_force_is_never_poisoned() {
    let once = Once::new();
    let mut poisoned = None;
    once.call_once_force(|state| poisoned = Some(state.is_poisoned()));
    assert_eq!(poisoned, Some(false));
    assert!(once.is_completed());
    // Waiting on a completed `Once` returns at once, and never panics.
    once.wait();
    once.wait_force();
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn waiters_block_until_completion() {
    // Waiters start before any initializer, so they sleep on an incomplete
    // `Once` that nobody runs yet.
    let once = Once::new();
    let slot = Slot::new();
    thread::scope(|s| {
        for i in 0..THREADS {
            let (once, slot) = (&once, &slot);
            s.spawn(move || {
                if i % 2 == 0 {
                    once.wait();
                } else {
                    once.wait_force();
                }
                assert!(once.is_completed());
                // SAFETY: waiting returned, so the write happened before.
                assert_eq!(unsafe { slot.get() }, 7);
            });
        }
        thread::sleep(Duration::from_millis(10));
        once.call_once(|| {
            // SAFETY: the waiters only read after completion.
            unsafe { slot.set(7) };
        });
    });
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn contended_initializer_wakes_callers() {
    // The initializer takes a while, so the other callers sleep on a
    // running `Once` and must be woken by its completion.
    let once = Once::new();
    let slot = Slot::new();
    thread::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                once.call_once(|| {
                    thread::sleep(Duration::from_millis(10));
                    // SAFETY: only the initializer writes, before the
                    // `Once` completes.
                    unsafe { slot.set(9) };
                });
                // SAFETY: `call_once` returned, so the write happened before.
                assert_eq!(unsafe { slot.get() }, 9);
            });
        }
    });
}

#[test]
fn nested_onces() {
    static OUTER: Once = Once::new();
    static INNER: Once = Once::new();
    let runs = AtomicUsize::new(0);
    OUTER.call_once(|| {
        INNER.call_once(|| {
            runs.fetch_add(1, Relaxed);
        });
    });
    assert!(OUTER.is_completed());
    assert!(INNER.is_completed());
    assert_eq!(runs.load(Relaxed), 1);
}

/// The initializer may query its own `Once`, which is not complete yet;
/// calling `call_once` on it again would deadlock instead.
#[test]
fn initializer_may_query_its_once() {
    static ONCE: Once = Once::new();
    let mut seen = None;
    ONCE.call_once(|| seen = Some(ONCE.is_completed()));
    assert_eq!(seen, Some(false));
    assert!(ONCE.is_completed());
}

#[test]
fn default_is_incomplete_and_runs_once() {
    let once = Once::default();
    assert!(!once.is_completed());
    let mut runs = 0;
    once.call_once(|| runs += 1);
    once.call_once(|| runs += 1);
    assert_eq!(runs, 1);
    assert!(once.is_completed());
}
