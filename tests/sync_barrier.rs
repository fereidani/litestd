//! `Barrier`: one leader per generation, reuse across generations, groups
//! of more threads than the barrier size, and memory visibility.
#![cfg(feature = "sync")]

use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
};
use std::thread;

use litestd::sync::Barrier;

const THREADS: usize = if cfg!(miri) { 3 } else { 8 };
const ROUNDS: usize = if cfg!(miri) { 4 } else { 1_000 };

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn one_leader_per_generation() {
    let barrier = Barrier::new(THREADS);
    let leaders = AtomicUsize::new(0);
    thread::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                for _ in 0..ROUNDS {
                    if barrier.wait().is_leader() {
                        leaders.fetch_add(1, Relaxed);
                    }
                }
            });
        }
    });
    assert_eq!(leaders.load(Relaxed), ROUNDS);
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn more_threads_than_the_barrier_size() {
    // Two groups of `THREADS` pass a barrier of size `THREADS`: each group
    // forms one generation with one leader.
    let barrier = Barrier::new(THREADS);
    let leaders = AtomicUsize::new(0);
    thread::scope(|s| {
        for _ in 0..2 * THREADS {
            s.spawn(|| {
                if barrier.wait().is_leader() {
                    leaders.fetch_add(1, Relaxed);
                }
            });
        }
    });
    assert_eq!(leaders.load(Relaxed), 2);
}

#[test]
fn small_barriers_never_block() {
    for n in [0, 1] {
        let barrier = Barrier::new(n);
        for _ in 0..3 {
            assert!(barrier.wait().is_leader());
        }
    }
}

/// One unsynchronized slot per thread: each thread writes its own before
/// the barrier and reads all of them after it, so Miri reports a data race
/// if the barrier does not order the writes before the reads.
struct Slots([UnsafeCell<usize>; THREADS]);

// SAFETY: the accesses in `makes_writes_visible` are ordered by the barrier
// under test; a missing order is the data race the test exists to catch.
unsafe impl Sync for Slots {}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn makes_writes_visible() {
    let barrier = Barrier::new(THREADS);
    let slots = Slots(core::array::from_fn(|_| UnsafeCell::new(0)));
    thread::scope(|s| {
        for i in 0..THREADS {
            let (barrier, slots) = (&barrier, &slots);
            s.spawn(move || {
                // SAFETY: only thread `i` writes slot `i`, and nobody reads
                // it before the barrier.
                unsafe { *slots.0[i].get() = i + 1 };
                let _ = barrier.wait();
                let sum: usize = slots
                    .0
                    .iter()
                    // SAFETY: every write happened before the barrier.
                    .map(|slot| unsafe { *slot.get() })
                    .sum();
                assert_eq!(sum, THREADS * (THREADS + 1) / 2);
            });
        }
    });
}
