//! The type-independent lock behind [`RwLock`](super::RwLock).
//!
//! `state` holds a 30-bit reader count (all ones for a writer) and two
//! waiting flags. Readers sleep on `state`, writers on `writer_seq`, which
//! an unlock increments to wake one. Writers go first: new readers wait
//! while a writer waits, and an unlock wakes a writer before any reader;
//! readers are woken together once no writer is left.
//!
//! `writers` counts waiting writers exactly, so a handoff never depends on
//! the futex reporting a woken thread (Windows never does).
//! `WRITERS_WAITING` only holds readers back and sends unlocks to the slow
//! path; it may be stale, and the slow path clears it.

use core::{
    hint,
    sync::atomic::{
        AtomicU32,
        Ordering::{Acquire, Relaxed, Release, SeqCst},
    },
};

use super::SPIN_LIMIT;
use crate::sys::futex;

/// The reader count, or `WRITE_LOCKED`.
const MASK: u32 = (1 << 30) - 1;
/// One reader.
const READER: u32 = 1;
/// A writer holds the lock.
const WRITE_LOCKED: u32 = MASK;
/// The most readers the count holds; further readers wait.
const MAX_READERS: u32 = MASK - 1;
/// Readers sleep on `state`, and an unlock must wake them.
const READERS_WAITING: u32 = 1 << 30;
/// Writers may sleep on `writer_seq`, and an unlock must check.
const WRITERS_WAITING: u32 = 1 << 31;

const fn is_unlocked(state: u32) -> bool {
    state & MASK == 0
}

const fn is_write_locked(state: u32) -> bool {
    state & MASK == WRITE_LOCKED
}

const fn has_readers_waiting(state: u32) -> bool {
    state & READERS_WAITING != 0
}

const fn has_writers_waiting(state: u32) -> bool {
    state & WRITERS_WAITING != 0
}

/// Whether `state` is unlocked with threads waiting: no reader or writer,
/// and a waiting flag set. Rotating the two flags to the bottom maps exactly
/// these states to 1, 2 and 3, so one comparison decides it.
const fn is_unlocked_with_waiters(state: u32) -> bool {
    let waiters = state.rotate_left(2).wrapping_sub(1) < 3;
    debug_assert!(
        waiters
            == (is_unlocked(state)
                && state & (READERS_WAITING | WRITERS_WAITING) != 0)
    );
    waiters
}

/// Whether a new reader may take the lock. Waiting readers block new ones as
/// well: they are only there because a writer went or goes first.
const fn is_read_lockable(state: u32) -> bool {
    state & MASK < MAX_READERS
        && state & (READERS_WAITING | WRITERS_WAITING) == 0
}

/// Whether a woken reader may take the lock: it joins current readers even
/// when writers wait, since readers are only woken when no writer is left
/// or by a downgrade that shares the lock with them.
const fn is_read_lockable_after_wakeup(state: u32) -> bool {
    let count = state & MASK;
    count != 0 && count < MAX_READERS && !has_readers_waiting(state)
}

/// A reader-writer lock without data.
pub(crate) struct RawRwLock {
    state: AtomicU32,
    /// Incremented to wake a writer; writers sleep on it.
    writer_seq: AtomicU32,
    /// Writers registered in `write_contended` that have not taken the lock.
    writers: AtomicU32,
}

impl RawRwLock {
    #[inline]
    pub(crate) const fn new() -> Self {
        Self {
            state: AtomicU32::new(0),
            writer_seq: AtomicU32::new(0),
            writers: AtomicU32::new(0),
        }
    }

    /// Acquires a read lock if possible, without blocking.
    #[cfg(feature = "sync")]
    #[inline]
    pub(crate) fn try_read(&self) -> bool {
        let mut state = self.state.load(Relaxed);
        // Retries only after a spurious failure or another thread's
        // successful update of the state, both finite.
        while is_read_lockable(state) {
            // Acquire: see the writes of the last writer.
            match self.state.compare_exchange_weak(
                state,
                state + READER,
                Acquire,
                Relaxed,
            ) {
                Ok(_) => return true,
                Err(s) => state = s,
            }
        }
        false
    }

    /// Acquires a read lock, blocking the current thread until it can.
    #[inline]
    pub(crate) fn read(&self) {
        let state = self.state.load(Relaxed);
        if !is_read_lockable(state)
            || self
                .state
                .compare_exchange_weak(state, state + READER, Acquire, Relaxed)
                .is_err()
        {
            self.read_contended();
        }
    }

    #[cold]
    #[inline(never)]
    fn read_contended(&self) {
        let mut has_slept = false;
        let mut state = self.spin_until(|s| {
            !is_write_locked(s) || s & (READERS_WAITING | WRITERS_WAITING) != 0
        });
        // Blocks until a read lock is taken. A reader sleeps only with
        // `READERS_WAITING` set, and every unlock that finds the flag with no
        // writer left clears it and wakes all readers, so each sleep ends.
        loop {
            let lockable = is_read_lockable(state)
                || (has_slept && is_read_lockable_after_wakeup(state));
            if lockable {
                match self.state.compare_exchange_weak(
                    state,
                    state + READER,
                    Acquire,
                    Relaxed,
                ) {
                    Ok(_) => return,
                    Err(s) => {
                        state = s;
                        continue;
                    }
                }
            }
            if !has_readers_waiting(state) {
                if let Err(s) = self.state.compare_exchange(
                    state,
                    state | READERS_WAITING,
                    Relaxed,
                    Relaxed,
                ) {
                    state = s;
                    continue;
                }
            }
            futex::wait(&self.state, state | READERS_WAITING);
            has_slept = true;
            state = self.spin_until(|s| {
                !is_write_locked(s)
                    || s & (READERS_WAITING | WRITERS_WAITING) != 0
            });
        }
    }

    /// Releases a read lock.
    ///
    /// # Safety
    ///
    /// The caller must hold a read lock and give it up.
    #[inline]
    pub(crate) unsafe fn read_unlock(&self) {
        // Release: the next writer must see that this reader is done.
        let state = self.state.fetch_sub(READER, Release) - READER;
        // The last reader wakes the waiters: a writer, or readers held back
        // by a full count.
        if is_unlocked_with_waiters(state) {
            self.wake_writer_or_readers(state);
        }
    }

    /// Acquires the write lock if it is free, without blocking.
    #[cfg(feature = "sync")]
    #[inline]
    pub(crate) fn try_write(&self) -> bool {
        let mut state = self.state.load(Relaxed);
        // Retries as in `try_read`.
        while is_unlocked(state) {
            match self.state.compare_exchange_weak(
                state,
                state | WRITE_LOCKED,
                Acquire,
                Relaxed,
            ) {
                Ok(_) => return true,
                Err(s) => state = s,
            }
        }
        false
    }

    /// Acquires the write lock, blocking the current thread until it can.
    #[inline]
    pub(crate) fn write(&self) {
        if self
            .state
            .compare_exchange_weak(0, WRITE_LOCKED, Acquire, Relaxed)
            .is_err()
        {
            self.write_contended();
        }
    }

    #[cold]
    #[inline(never)]
    fn write_contended(&self) {
        let mut registered = false;
        let mut state = self.spin_write();
        // Blocks until this thread takes the lock. A writer sleeps only
        // registered, with `WRITERS_WAITING` set on a held lock, whose unlock
        // increments `writer_seq` and wakes a registered writer, so some
        // registered writer retries after each unlock and each one wins.
        loop {
            if is_unlocked(state) {
                // A registered writer keeps `WRITERS_WAITING` set: others may
                // sleep, and the flag makes this thread's unlock wake them.
                let flag = if registered { WRITERS_WAITING } else { 0 };
                match self.state.compare_exchange_weak(
                    state,
                    state | WRITE_LOCKED | flag,
                    Acquire,
                    Relaxed,
                ) {
                    Ok(_) => {
                        if registered {
                            self.writers.fetch_sub(1, Relaxed);
                        }
                        return;
                    }
                    Err(s) => {
                        state = s;
                        continue;
                    }
                }
            }
            if !registered {
                // SeqCst: pairs with `wake_writer`, see there.
                self.writers.fetch_add(1, SeqCst);
                registered = true;
            }
            if !has_writers_waiting(state) {
                if let Err(s) = self.state.compare_exchange(
                    state,
                    state | WRITERS_WAITING,
                    Relaxed,
                    Relaxed,
                ) {
                    state = s;
                    continue;
                }
            }
            // Sample the sequence before the final state check, so an unlock
            // in between changes it and ends the sleep. SeqCst also makes the
            // sample `Acquire`: seeing an unlock's increment means seeing
            // that unlock clear `WRITERS_WAITING` below.
            let seq = self.writer_seq.load(SeqCst);
            state = self.state.load(Relaxed);
            if is_unlocked(state) || !has_writers_waiting(state) {
                continue;
            }
            futex::wait(&self.writer_seq, seq);
            state = self.spin_write();
        }
    }

    /// Spins until the lock is free or other writers wait: queueing behind
    /// waiting writers is fairer than racing them.
    fn spin_write(&self) -> u32 {
        self.spin_until(|s| is_unlocked(s) || has_writers_waiting(s))
    }

    /// Releases the write lock.
    ///
    /// # Safety
    ///
    /// The caller must hold the write lock and give it up.
    #[inline]
    pub(crate) unsafe fn write_unlock(&self) {
        // Release: publish the critical section to the next owner.
        let state = self.state.fetch_sub(WRITE_LOCKED, Release) - WRITE_LOCKED;
        debug_assert!(is_unlocked(state));
        if state & (READERS_WAITING | WRITERS_WAITING) != 0 {
            self.wake_writer_or_readers(state);
        }
    }

    /// Turns the write lock into a read lock, letting waiting readers in.
    ///
    /// # Safety
    ///
    /// The caller must hold the write lock; it holds a read lock afterwards.
    #[cfg(feature = "sync")]
    #[inline]
    pub(crate) unsafe fn downgrade(&self) {
        // Release: readers that join now see the critical section.
        let state = self.state.fetch_sub(WRITE_LOCKED - READER, Release);
        debug_assert!(is_write_locked(state));
        if has_readers_waiting(state) {
            self.wake_readers_after_downgrade();
        }
    }

    #[cfg(feature = "sync")]
    #[cold]
    #[inline(never)]
    fn wake_readers_after_downgrade(&self) {
        // Only an unlock of the whole lock clears the flag otherwise, and
        // this thread keeps a read lock, so no other thread clears it now.
        self.state.fetch_and(!READERS_WAITING, Relaxed);
        futex::wake_all(&self.state);
    }

    /// Wakes the waiters of a lock that was just unlocked: one writer if any
    /// is registered, otherwise all readers. A thread that takes the lock
    /// meanwhile inherits the waiters and wakes them when it unlocks.
    #[cold]
    #[inline(never)]
    fn wake_writer_or_readers(&self, mut state: u32) {
        // Repeats only when another thread changed the state after it was
        // read: a waiter set a flag, which happens at most once per flag
        // before this thread clears it, or another unlock handled it.
        loop {
            if !is_unlocked(state) {
                return;
            }
            if has_writers_waiting(state) {
                // Readers stay asleep with `READERS_WAITING` kept: the
                // writer that takes the lock wakes them on its unlock.
                let cleared = state & !WRITERS_WAITING;
                if let Err(s) = self
                    .state
                    .compare_exchange(state, cleared, Relaxed, Relaxed)
                {
                    state = s;
                    continue;
                }
                if self.wake_writer() {
                    return;
                }
                // The flag was stale: no writer waits. Fall back to readers.
                state = cleared;
            }
            if !has_readers_waiting(state) {
                return;
            }
            match self.state.compare_exchange(
                state,
                state & !READERS_WAITING,
                Relaxed,
                Relaxed,
            ) {
                Ok(_) => {
                    futex::wake_all(&self.state);
                    return;
                }
                Err(s) => state = s,
            }
        }
    }

    /// Wakes one waiting writer. Returns `false` if no writer is registered,
    /// in which case none is woken, and none sleeps unwoken.
    fn wake_writer(&self) -> bool {
        // SeqCst puts this increment, the load below and their counterparts
        // in `write_contended` (the registration, then the `writer_seq`
        // sample) in one total order. If the load misses a registration,
        // that writer samples after the increment, whose Release carries the
        // cleared `WRITERS_WAITING` to it, so it retries instead of sleeping.
        self.writer_seq.fetch_add(1, SeqCst);
        if self.writers.load(SeqCst) == 0 {
            return false;
        }
        // A registered writer sleeps on an older sequence and is woken here,
        // or has not slept yet and retries; the futex result is irrelevant.
        futex::wake(&self.writer_seq);
        true
    }

    /// Spins until `stop` accepts the state, for a bounded time, and returns
    /// the last state seen.
    #[inline]
    fn spin_until(&self, stop: impl Fn(u32) -> bool) -> u32 {
        for _ in 0..SPIN_LIMIT {
            let state = self.state.load(Relaxed);
            if stop(state) {
                return state;
            }
            hint::spin_loop();
        }
        self.state.load(Relaxed)
    }
}
