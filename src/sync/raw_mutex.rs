//! The type-independent lock behind `Mutex`, `Condvar` and `Barrier`, and
//! behind the standard streams of `io`.
//!
//! One futex word with three states. A thread that finds the lock taken
//! spins briefly, then marks it contended and sleeps; an unlock that sees
//! the mark wakes one sleeper, which marks the lock contended again when it
//! takes it, so a sleeper left behind is always woken by a later unlock.

use core::{
    hint,
    sync::atomic::{
        AtomicU32,
        Ordering::{Acquire, Relaxed, Release},
    },
};

use super::SPIN_LIMIT;
use crate::sys::futex;

/// Free.
const UNLOCKED: u32 = 0;
/// Held, and no thread sleeps on the futex.
const LOCKED: u32 = 1;
/// Held, and threads may sleep on the futex: unlocking must wake one.
const CONTENDED: u32 = 2;

/// A mutual exclusion lock without data.
pub(crate) struct RawMutex {
    state: AtomicU32,
}

impl RawMutex {
    #[inline]
    pub(crate) const fn new() -> Self {
        Self {
            state: AtomicU32::new(UNLOCKED),
        }
    }

    /// Acquires the lock if it is free, without blocking.
    #[inline]
    pub(crate) fn try_lock(&self) -> bool {
        // Acquire: see the writes the previous owner released on unlock.
        self.state
            .compare_exchange(UNLOCKED, LOCKED, Acquire, Relaxed)
            .is_ok()
    }

    /// Acquires the lock, blocking the current thread until it is free.
    #[inline]
    pub(crate) fn lock(&self) {
        if !self.try_lock() {
            self.lock_contended();
        }
    }

    #[cold]
    #[inline(never)]
    fn lock_contended(&self) {
        let mut state = self.spin();
        if state == UNLOCKED {
            // Freed during the spin: take it without the contended mark,
            // which would cost the next unlock a needless system call.
            match self
                .state
                .compare_exchange(UNLOCKED, LOCKED, Acquire, Relaxed)
            {
                Ok(_) => return,
                Err(s) => state = s,
            }
        }
        // Blocks until the lock is released: every unlock of a contended
        // lock wakes a sleeper, which retries, until this thread wins.
        loop {
            // Marking the lock contended also takes it if it was free. Skip
            // the write if the mark is there, keeping the cache line shared.
            if state != CONTENDED
                && self.state.swap(CONTENDED, Acquire) == UNLOCKED
            {
                return;
            }
            futex::wait(&self.state, CONTENDED);
            state = self.spin();
        }
    }

    /// Spins while the lock is held and no thread sleeps, returning the last
    /// state seen. It stops on `CONTENDED` too: queueing behind sleepers is
    /// fairer than racing them.
    fn spin(&self) -> u32 {
        for _ in 0..SPIN_LIMIT {
            // A plain load keeps the cache line shared while spinning.
            let state = self.state.load(Relaxed);
            if state != LOCKED {
                return state;
            }
            hint::spin_loop();
        }
        self.state.load(Relaxed)
    }

    /// Releases the lock.
    ///
    /// # Safety
    ///
    /// The caller must hold the lock, or act for the thread that does, and
    /// give up the access it protects.
    #[inline]
    pub(crate) unsafe fn unlock(&self) {
        // Release: publish the critical section to the next owner.
        if self.state.swap(UNLOCKED, Release) == CONTENDED {
            self.wake();
        }
    }

    #[cold]
    #[inline(never)]
    fn wake(&self) {
        // One suffices: it marks the lock contended again when it takes it,
        // so its unlock wakes the next. Whether one was woken is irrelevant.
        futex::wake(&self.state);
    }
}
