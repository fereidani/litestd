//! [`Barrier`] and [`BarrierWaitResult`].
//!
//! Arrivals are counted under a raw mutex; the last one starts a new
//! generation in a futex word that the others wait on. Unlike std, waiters
//! do not retake the mutex after the wakeup, and the last thread makes no
//! system call while every waiter is still spinning.

#[cfg(target_vendor = "apple")]
use core::marker::PhantomData;
use core::{
    fmt, hint,
    sync::atomic::{
        AtomicU32, AtomicUsize,
        Ordering::{Acquire, Relaxed, Release},
    },
};

use super::{SPIN_LIMIT, raw_mutex::RawMutex};
use crate::sys::futex;

/// Set in `generation` while threads may sleep on it.
const SLEEPING: u32 = 1;
/// One step of the generation counter, above the `SLEEPING` bit.
const GENERATION: u32 = 2;

/// A barrier enables multiple threads to synchronize the beginning of some
/// computation.
pub struct Barrier {
    lock: RawMutex,
    /// The threads that arrived in the current generation. Only accessed
    /// with `lock` held, so `Relaxed` suffices.
    count: AtomicUsize,
    /// The generation counter in steps of `GENERATION`, plus `SLEEPING`.
    /// Its generation part only changes with `lock` held.
    generation: AtomicU32,
    num_threads: usize,
    /// std's `Barrier` is `!UnwindSafe` on macOS, where its lock holds
    /// pointers to pthread objects; this keeps the same auto traits there.
    #[cfg(target_vendor = "apple")]
    _marker: PhantomData<&'static mut ()>,
}

/// A `BarrierWaitResult` is returned by [`Barrier::wait()`] when all threads
/// in the [`Barrier`] have rendezvoused.
pub struct BarrierWaitResult(bool);

impl Barrier {
    /// Creates a new barrier that blocks `n - 1` threads calling
    /// [`wait()`](Barrier::wait) and wakes them all when the `n`th calls it.
    #[must_use]
    #[inline]
    pub const fn new(n: usize) -> Self {
        Self {
            lock: RawMutex::new(),
            count: AtomicUsize::new(0),
            generation: AtomicU32::new(0),
            num_threads: n,
            #[cfg(target_vendor = "apple")]
            _marker: PhantomData,
        }
    }

    /// Blocks the current thread until all threads have rendezvoused here.
    ///
    /// The barrier is reusable, one arbitrary thread per generation leads,
    /// and every write a thread makes before `wait` is visible to the whole
    /// generation after `wait` returns.
    pub fn wait(&self) -> BarrierWaitResult {
        self.lock.lock();
        // With the lock held, the generation part is stable; only the
        // `SLEEPING` bit changes concurrently.
        let generation = self.generation.load(Relaxed) & !SLEEPING;
        let arrived = self.count.load(Relaxed) + 1;
        if arrived < self.num_threads {
            self.count.store(arrived, Relaxed);
            // SAFETY: this thread took the lock above.
            unsafe { self.lock.unlock() };
            self.wait_for_generation(generation);
            BarrierWaitResult(false)
        } else {
            self.count.store(0, Relaxed);
            // Release: every arrival's writes reached this thread through
            // the lock, and the waiters acquire them from this store.
            let prev = self
                .generation
                .swap(generation.wrapping_add(GENERATION), Release);
            // SAFETY: this thread took the lock above.
            unsafe { self.lock.unlock() };
            if prev & SLEEPING != 0 {
                futex::wake_all(&self.generation);
            }
            BarrierWaitResult(true)
        }
    }

    /// Blocks until the generation part of `generation` moves past
    /// `current`.
    fn wait_for_generation(&self, current: u32) {
        // The last thread often arrives within microseconds, so a short spin
        // saves both system calls. Longer spins won with four to eight
        // threads on eight cores but lost twice over once threads
        // outnumbered cores, keeping the last thread off the CPU.
        for _ in 0..SPIN_LIMIT {
            // Acquire: see the writes the other threads made before `wait`.
            if self.generation.load(Acquire) & !SLEEPING != current {
                return;
            }
            hint::spin_loop();
        }
        // Blocks until the last thread of this generation swaps in the next
        // one and, seeing `SLEEPING`, wakes every sleeper. A thread misses
        // its release only if 2^31 generations pass between two loads.
        loop {
            let state = self.generation.load(Acquire);
            if state & !SLEEPING != current {
                return;
            }
            if state & SLEEPING == 0
                && self
                    .generation
                    .compare_exchange(state, state | SLEEPING, Relaxed, Relaxed)
                    .is_err()
            {
                continue;
            }
            futex::wait(&self.generation, current | SLEEPING);
        }
    }
}

impl fmt::Debug for Barrier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Barrier").finish_non_exhaustive()
    }
}

impl BarrierWaitResult {
    /// Returns `true` if this thread is the "leader thread" for the call to
    /// [`Barrier::wait()`]; exactly one thread per generation is.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn is_leader(&self) -> bool {
        self.0
    }
}

impl fmt::Debug for BarrierWaitResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BarrierWaitResult")
            .field("is_leader", &self.0)
            .finish()
    }
}
