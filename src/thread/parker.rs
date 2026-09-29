//! The token behind `park` and `unpark`, on top of `sys::futex`.

use core::{
    sync::atomic::{
        AtomicU32,
        Ordering::{Acquire, Relaxed, Release},
    },
    time::Duration,
};

use crate::sys::{futex, time};

/// No token, and the owner is not parked.
const EMPTY: u32 = 0;
/// The token is available.
const NOTIFIED: u32 = 1;
/// The owner is parked or about to wait. `EMPTY - 1`, so that the
/// `fetch_sub` that consumes a token also moves `EMPTY` to `PARKED`.
const PARKED: u32 = u32::MAX;

/// A thread's parking token. Only the owning thread parks; any thread
/// unparks.
///
/// `unpark` stores the token with `Release` and the `park` that consumes it
/// loads it with `Acquire`, so everything before an `unpark` happens before
/// the `park` it ends returns. Every `unpark` writes the state, even when
/// the token is already there, so each call takes part in that ordering.
pub(crate) struct Parker {
    state: AtomicU32,
}

impl Parker {
    pub(crate) const fn new() -> Self {
        Self {
            state: AtomicU32::new(EMPTY),
        }
    }

    /// Blocks until the token is available, then consumes it. Only the
    /// owning thread may call this.
    pub(crate) fn park(&self) {
        // Consumes a token (NOTIFIED to EMPTY) or announces the wait (EMPTY
        // to PARKED).
        if self.state.fetch_sub(1, Acquire) == NOTIFIED {
            return;
        }
        // Waits for another thread's `unpark`. A wakeup that finds no token
        // is spurious and waits again; the loop ends once `unpark` stored
        // the token, which is what the caller is waiting for.
        loop {
            futex::wait(&self.state, PARKED);
            if self
                .state
                .compare_exchange(NOTIFIED, EMPTY, Acquire, Relaxed)
                .is_ok()
            {
                return;
            }
        }
    }

    /// Blocks until the token is available or roughly `timeout` elapsed,
    /// then consumes any token. Only the owning thread may call this.
    pub(crate) fn park_timeout(&self, timeout: Duration) {
        if self.state.fetch_sub(1, Acquire) == NOTIFIED {
            return;
        }
        // A timeout, a spurious wakeup and an `unpark` all end the wait:
        // `park_timeout` may return early, so whether the deadline passed
        // does not matter. One `Duration` cannot hold lies billions of years
        // away and is treated as none.
        if let Some(deadline) = time::monotonic().checked_add(timeout) {
            futex::wait_until(&self.state, PARKED, deadline);
        } else {
            futex::wait(&self.state, PARKED);
        }
        // A swap, not a store, so that a token stored meanwhile is acquired.
        self.state.swap(EMPTY, Acquire);
    }

    /// Makes the token available, waking the owner if it is parked.
    #[inline]
    pub(crate) fn unpark(&self) {
        // Only a parked owner needs the system call.
        if self.state.swap(NOTIFIED, Release) == PARKED {
            futex::wake(&self.state);
        }
    }
}
