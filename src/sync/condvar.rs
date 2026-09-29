//! [`Condvar`] and [`WaitTimeoutResult`].
//!
//! A waiter registers in `waiters` and samples `seq` while it holds the
//! mutex, then unlocks it and sleeps on `seq` until a notification changes
//! it. A notification that finds no waiter is a single load, where std's
//! futex condvar always makes a system call.
//!
//! The count cannot miss a sleeper. A notifier that changed the predicate
//! under the mutex after the waiter unlocked it synchronizes with that
//! unlock, so it sees the registration, and its increment of `seq` follows
//! the sample: the futex wait returns at once or is woken by the wake that
//! follows. A notifier that changed it before the waiter locked the mutex
//! needs no wake, as the waiter sees the new predicate. std relies on the
//! same ordering, so every access here is `Relaxed`. A notifier that changes
//! the predicate without the mutex can lose the wakeup with std's condvar
//! too, since the waiter may sample `seq` after the increment.

use core::{
    fmt,
    sync::atomic::{AtomicU32, Ordering::Relaxed},
    time::Duration,
};

use super::{mutex::MutexGuard, poison::LockResult, raw_mutex::RawMutex};
use crate::sys::{futex, time};

/// A type indicating whether a timed wait on a condition variable returned
/// due to a time out or not.
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub struct WaitTimeoutResult(bool);

impl WaitTimeoutResult {
    /// Returns `true` if the wait was known to have timed out.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn timed_out(&self) -> bool {
        self.0
    }
}

/// A condition variable.
///
/// Waiting blocks the current thread. Using more than one mutex with the
/// same condition variable may lose notifications.
pub struct Condvar {
    /// Changed by every notification that finds a waiter; waiters sleep on
    /// it until it differs from their sample.
    seq: AtomicU32,
    /// The number of threads between registration and wakeup in a wait.
    waiters: AtomicU32,
}

impl Condvar {
    /// Creates a new condition variable ready to be waited on and notified.
    #[must_use]
    #[inline]
    pub const fn new() -> Self {
        Self {
            seq: AtomicU32::new(0),
            waiters: AtomicU32::new(0),
        }
    }

    /// Blocks the current thread until this condition variable receives a
    /// notification, unlocking the mutex of `guard` meanwhile. Spurious
    /// wakeups are possible, so recheck the predicate.
    ///
    /// # Errors
    ///
    /// Never fails: litestd mutexes are never poisoned.
    #[inline]
    pub fn wait<'a, T>(
        &self,
        guard: MutexGuard<'a, T>,
    ) -> LockResult<MutexGuard<'a, T>> {
        self.wait_raw(MutexGuard::raw(&guard), None);
        Ok(guard)
    }

    /// Blocks the current thread until `condition`, which runs with the lock
    /// held, returns `false`.
    ///
    /// # Errors
    ///
    /// Never fails: litestd mutexes are never poisoned.
    #[inline]
    pub fn wait_while<'a, T, F>(
        &self,
        mut guard: MutexGuard<'a, T>,
        mut condition: F,
    ) -> LockResult<MutexGuard<'a, T>>
    where
        F: FnMut(&mut T) -> bool,
    {
        // The mutex is locked whenever `condition` runs, and `guard` unlocks
        // it if `condition` unwinds.
        while condition(&mut *guard) {
            self.wait_raw(MutexGuard::raw(&guard), None);
        }
        Ok(guard)
    }

    /// Waits on this condition variable for a notification, timing out
    /// after roughly `dur`. Spurious wakeups are possible.
    ///
    /// # Errors
    ///
    /// Never fails: litestd mutexes are never poisoned.
    #[inline]
    pub fn wait_timeout<'a, T>(
        &self,
        guard: MutexGuard<'a, T>,
        dur: Duration,
    ) -> LockResult<(MutexGuard<'a, T>, WaitTimeoutResult)> {
        let woken = self.wait_raw(MutexGuard::raw(&guard), deadline_after(dur));
        Ok((guard, WaitTimeoutResult(!woken)))
    }

    /// Like [`wait_while`](Self::wait_while), but gives up after roughly
    /// `dur`.
    ///
    /// # Errors
    ///
    /// Never fails: litestd mutexes are never poisoned.
    #[inline]
    pub fn wait_timeout_while<'a, T, F>(
        &self,
        mut guard: MutexGuard<'a, T>,
        dur: Duration,
        mut condition: F,
    ) -> LockResult<(MutexGuard<'a, T>, WaitTimeoutResult)>
    where
        F: FnMut(&mut T) -> bool,
    {
        // One deadline for every wait: the clock is read once, and a wait
        // reports when the deadline passed. As in std, the result is a
        // timeout only if `condition` still holds afterwards.
        let deadline = deadline_after(dur);
        let mut timed_out = false;
        while condition(&mut *guard) {
            if timed_out {
                return Ok((guard, WaitTimeoutResult(true)));
            }
            timed_out = !self.wait_raw(MutexGuard::raw(&guard), deadline);
        }
        Ok((guard, WaitTimeoutResult(false)))
    }

    /// Wakes up one blocked thread on this condvar. With no blocked thread,
    /// this is a single load.
    #[inline]
    pub fn notify_one(&self) {
        if self.waiters.load(Relaxed) != 0 {
            self.notify(false);
        }
    }

    /// Wakes up all blocked threads on this condvar. With no blocked thread,
    /// this is a single load.
    #[inline]
    pub fn notify_all(&self) {
        if self.waiters.load(Relaxed) != 0 {
            self.notify(true);
        }
    }

    #[cold]
    #[inline(never)]
    fn notify(&self, all: bool) {
        // The increment makes a waiter that sampled `seq` but is not yet
        // asleep return from its futex wait at once.
        self.seq.fetch_add(1, Relaxed);
        if all {
            futex::wake_all(&self.seq);
        } else {
            // Whether a thread was actually woken does not matter.
            futex::wake(&self.seq);
        }
    }

    /// Unlocks `mutex`, waits for a notification or the absolute
    /// `deadline` of `time::monotonic`, and locks `mutex` again; returns
    /// `false` only if the deadline passed. The caller's guard is untouched
    /// meanwhile. Outlined, so that the generic callers stay small enough to
    /// inline and keep their `Ok` visible to the optimizer.
    #[cold]
    #[inline(never)]
    fn wait_raw(&self, mutex: &RawMutex, deadline: Option<Duration>) -> bool {
        // Register and sample while the mutex is held; the module
        // documentation shows why this cannot lose a notification.
        self.waiters.fetch_add(1, Relaxed);
        let seq = self.seq.load(Relaxed);
        // SAFETY: the caller's guard holds the lock and is not used until
        // the lock is taken again below, before this function returns.
        unsafe { mutex.unlock() };
        let woken = deadline.map_or_else(
            || {
                futex::wait(&self.seq, seq);
                true
            },
            |deadline| futex::wait_until(&self.seq, seq, deadline),
        );
        self.waiters.fetch_sub(1, Relaxed);
        mutex.lock();
        woken
    }
}

/// Returns the deadline `dur` from now, or `None` for one that `Duration`
/// cannot hold, billions of years away, which the waits treat as none.
fn deadline_after(dur: Duration) -> Option<Duration> {
    time::monotonic().checked_add(dur)
}

impl fmt::Debug for Condvar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Condvar").finish_non_exhaustive()
    }
}

impl Default for Condvar {
    /// Creates a `Condvar` which is ready to be waited on and notified.
    fn default() -> Self {
        Self::new()
    }
}
