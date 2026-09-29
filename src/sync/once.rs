//! [`Once`] and [`OnceState`], and the claim-and-complete protocol that
//! [`OnceLock`](super::OnceLock) and [`LazyLock`](super::LazyLock) build on.
//!
//! The state is one futex word. Claiming moves it from `INCOMPLETE` to
//! `RUNNING` and yields an [`Init`] token, whose completion publishes
//! `COMPLETE` and wakes the threads that set `QUEUED` and sleep. An
//! unwinding initializer aborts the process, so nothing is poisoned. The
//! initializer runs in the caller's generic code, not behind a `dyn FnMut`.

use core::{
    cell::Cell,
    fmt,
    marker::PhantomData,
    sync::atomic::{
        AtomicU32,
        Ordering::{Acquire, Relaxed, Release},
    },
};

use crate::{sys::futex, unwind::abort_on_unwind};

// `INCOMPLETE` is zero so that a `static` `Once` or `OnceLock` needs no
// file space: it is placed in the zero-initialized section.

/// No initialization has run, and no thread is running one.
const INCOMPLETE: u32 = 0;
/// A thread is running the initialization.
const RUNNING: u32 = 1;
/// The initialization completed. Never combined with `QUEUED`.
const COMPLETE: u32 = 2;
/// Selects the state above.
const STATE_MASK: u32 = 3;
/// Threads sleep on the futex and must be woken on completion.
const QUEUED: u32 = 4;

/// A low-level synchronization primitive for one-time global execution.
///
/// Prefer [`OnceLock`] when the `Once` is associated with data. A litestd
/// `Once` is never poisoned: a panicking initializer terminates the
/// process. std implements `Default` for it only from Rust 1.100.
///
/// [`OnceLock`]: super::OnceLock
pub struct Once {
    state: AtomicU32,
}

/// State yielded to [`Once::call_once_force()`]'s closure parameter; it
/// never reports poisoning in litestd.
pub struct OnceState {
    // std's `OnceState` holds a `Cell`, which makes it `!Sync` and
    // `!RefUnwindSafe`; keep the same auto traits.
    _marker: PhantomData<StateMarker>,
}

/// What std's `OnceState` holds in its `Cell`: on macOS a pointer, which
/// also makes it `!Send`.
#[cfg(not(target_vendor = "apple"))]
type StateMarker = Cell<()>;
#[cfg(target_vendor = "apple")]
type StateMarker = Cell<*mut ()>;

/// The claim on a [`Once`]'s initialization: its holder alone runs the
/// initializer, then calls [`Init::complete`]. It needs no destructor: the
/// initializer runs inside `abort_on_unwind`, so every claim completes.
#[must_use = "a claimed initialization must be completed"]
pub(super) struct Init<'a> {
    once: &'a Once,
}

impl Once {
    /// Creates a new `Once` value.
    #[inline]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: AtomicU32::new(INCOMPLETE),
        }
    }

    /// Creates a `Once` whose initialization has already completed.
    #[inline]
    pub(super) const fn new_complete() -> Self {
        Self {
            state: AtomicU32::new(COMPLETE),
        }
    }

    /// Performs an initialization routine once and only once, blocking while
    /// another thread runs one. On return, some initialization has completed
    /// and its writes are visible. Calling it again from `f` deadlocks, and
    /// a panic in `f` terminates the process.
    #[inline]
    pub fn call_once<F>(&self, f: F)
    where
        F: FnOnce(),
    {
        if !self.is_completed() {
            self.call_once_slow(f);
        }
    }

    #[cold]
    fn call_once_slow<F: FnOnce()>(&self, f: F) {
        if let Some(init) = self.claim() {
            abort_on_unwind(f);
            init.complete();
        }
    }

    /// Performs the same function as [`call_once()`](Once::call_once) except
    /// ignores poisoning, which never happens in litestd; blocks likewise.
    #[inline]
    pub fn call_once_force<F>(&self, f: F)
    where
        F: FnOnce(&OnceState),
    {
        self.call_once(|| {
            f(&OnceState {
                _marker: PhantomData,
            });
        });
    }

    /// Returns `true` if some [`call_once()`](Once::call_once) call has
    /// completed successfully. A `false` result may already be stale.
    #[inline]
    pub fn is_completed(&self) -> bool {
        // Acquire: a caller that sees `COMPLETE` sees the initializer's
        // writes, published by the `Release` in `Init::complete`.
        self.state.load(Acquire) == COMPLETE
    }

    /// Blocks the current thread until initialization has completed.
    #[inline]
    pub fn wait(&self) {
        if !self.is_completed() {
            self.wait_slow();
        }
    }

    /// Blocks the current thread until initialization has completed,
    /// ignoring poisoning; the same as [`wait`](Self::wait) in litestd.
    #[inline]
    pub fn wait_force(&self) {
        self.wait();
    }

    /// Returns `true` if initialization has completed, without an atomic
    /// operation.
    #[inline]
    pub(super) fn is_completed_mut(&mut self) -> bool {
        // With exclusive access no initializer runs and no thread sleeps,
        // so the state is `INCOMPLETE` or `COMPLETE`.
        let state = *self.state.get_mut();
        debug_assert!(state == INCOMPLETE || state == COMPLETE);
        state == COMPLETE
    }

    /// Marks the initialization complete, without an atomic operation.
    #[inline]
    pub(super) fn complete_mut(&mut self) {
        *self.state.get_mut() = COMPLETE;
    }

    /// Blocks until the initialization completes, returning `None` with the
    /// initializer's writes visible, or until this thread claims it.
    #[cold]
    pub(super) fn claim(&self) -> Option<Init<'_>> {
        let mut state = self.state.load(Acquire);
        // Blocks while another thread runs the initialization: it completes
        // and wakes the sleepers (or aborts the process), and this loop then
        // returns.
        loop {
            match state & STATE_MASK {
                COMPLETE => return None,
                INCOMPLETE => {
                    // Keep `QUEUED` for the completion to wake the sleepers.
                    // Acquire on failure: the new state may be `COMPLETE`.
                    match self.state.compare_exchange_weak(
                        state,
                        RUNNING | (state & QUEUED),
                        Relaxed,
                        Acquire,
                    ) {
                        Ok(_) => return Some(Init { once: self }),
                        Err(s) => state = s,
                    }
                }
                _ => state = self.sleep(state),
            }
        }
    }

    #[cold]
    fn wait_slow(&self) {
        let mut state = self.state.load(Acquire);
        // Blocks until some thread completes the initialization, which
        // wakes every sleeper.
        while state != COMPLETE {
            state = self.sleep(state);
        }
    }

    /// Sleeps until the state changes from the incomplete `state`, and
    /// returns the new state.
    fn sleep(&self, state: u32) -> u32 {
        if state & QUEUED == 0 {
            // Acquire on failure: the new state may be `COMPLETE`.
            if let Err(s) = self.state.compare_exchange_weak(
                state,
                state | QUEUED,
                Relaxed,
                Acquire,
            ) {
                return s;
            }
        }
        futex::wait(&self.state, state | QUEUED);
        self.state.load(Acquire)
    }
}

impl Init<'_> {
    /// Publishes the completed initialization and wakes its sleepers.
    #[inline]
    pub(super) fn complete(self) {
        // Release: threads that see `COMPLETE` with an `Acquire` load see
        // the initializer's writes.
        if self.once.state.swap(COMPLETE, Release) & QUEUED != 0 {
            futex::wake_all(&self.once.state);
        }
    }
}

impl Default for Once {
    /// Creates a new `Once` value, same as [`Once::new`].
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Once {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Once").finish_non_exhaustive()
    }
}

impl OnceState {
    /// Returns `true` if the associated [`Once`] was poisoned before the
    /// closure ran; always `false` in litestd.
    #[inline]
    // Neither `const` nor `#[must_use]`, as in std.
    #[allow(
        clippy::missing_const_for_fn,
        clippy::must_use_candidate,
        clippy::unused_self
    )]
    pub fn is_poisoned(&self) -> bool {
        false
    }
}

impl fmt::Debug for OnceState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OnceState")
            .field("poisoned", &false)
            .finish()
    }
}
