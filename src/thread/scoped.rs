//! Scoped threads.

use core::{
    fmt,
    marker::PhantomData,
    sync::atomic::{
        AtomicU32,
        Ordering::{Acquire, Relaxed, Release},
    },
};

use alloc_crate::sync::Arc;

use super::{
    Builder, Result, Thread, functions::spawn_failed, spawn, spawn::JoinInner,
};
use crate::{io, sys::futex, unwind::abort_on_unwind};

/// A scope to spawn scoped threads in; see [`scope`].
pub struct Scope<'scope, 'env: 'scope> {
    data: Arc<ScopeData>,
    /// Invariance over both lifetimes. `'scope` must not shrink, which is
    /// necessary for soundness: a thread spawned from a scoped thread could
    /// otherwise borrow that thread's locals.
    lifetimes: PhantomData<(&'scope mut &'scope (), &'env mut &'env ())>,
}

/// An owned permission to join on a scoped thread (block on its
/// termination).
pub struct ScopedJoinHandle<'scope, T>(JoinInner<'scope, T>);

// SAFETY: as for `JoinHandle`: the handle gives out a `T` only by value,
// from `join`, so it may move to other threads when `T` may. std derives
// the same bounds.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl<T: Send> Send for ScopedJoinHandle<'_, T> {}
// SAFETY: shared references only read the thread handle and an atomic
// count; std has the same bound.
unsafe impl<T: Send> Sync for ScopedJoinHandle<'_, T> {}

/// The number of a scope's threads that are not done yet: their closure
/// returned and their result was taken or dropped, plus `SLEEPING`. The
/// scope waits on it through the futex; the `Arc` lets the last thread
/// still wake the scope after the scope saw zero.
pub(super) struct ScopeData {
    running: AtomicU32,
}

/// Set in `running` while the scope may sleep on it: only then does the
/// last thread make the system call that wakes it.
const SLEEPING: u32 = 1 << 31;

/// The most threads a scope counts, far beyond what a system can run.
/// Leaked [`ScopedJoinHandle`]s stay counted forever, so this guards the
/// count against reaching `SLEEPING`.
const MAX_RUNNING: u32 = SLEEPING / 2;

impl ScopeData {
    /// Counts a thread that is about to start.
    pub(super) fn increment(&self) -> io::Result<()> {
        // The thread creation that follows orders this before the thread's
        // `decrement`, and a running scoped thread that spawns keeps the
        // count above zero, so zero comes after every increment.
        if self.running.fetch_add(1, Relaxed) & !SLEEPING >= MAX_RUNNING {
            self.decrement();
            return Err(too_many_threads());
        }
        Ok(())
    }

    /// Marks a thread as done.
    pub(super) fn decrement(&self) {
        // `Release` orders everything the thread did, including dropping its
        // result, before the scope observes zero with `Acquire`; every later
        // read-modify-write, the scope's included, continues the release
        // sequence.
        if self.running.fetch_sub(1, Release) == SLEEPING | 1 {
            futex::wake(&self.running);
        }
    }

    /// Blocks until every thread of the scope is done.
    fn wait(&self) {
        let mut running = self.running.load(Acquire);
        // Sleeps only with `SLEEPING` set, which makes the decrement to zero
        // wake the futex, so the loop ends once every thread is done;
        // wakeups that find threads running wait again.
        while running & !SLEEPING != 0 {
            if running & SLEEPING == 0 {
                // Acquire on failure: the new count may be zero.
                if let Err(now) = self.running.compare_exchange(
                    running,
                    running | SLEEPING,
                    Relaxed,
                    Acquire,
                ) {
                    running = now;
                    continue;
                }
            }
            futex::wait(&self.running, running | SLEEPING);
            running = self.running.load(Acquire);
        }
    }
}

#[cold]
const fn too_many_threads() -> io::Error {
    io::const_error!(
        io::ErrorKind::Other,
        "too many running threads in thread scope"
    )
}

/// Creates a scope for spawning scoped threads.
///
/// Scoped threads may borrow non-`'static` data: `'env` covers what they
/// borrow and outlives `'scope`, and this function blocks until every
/// thread of the scope is joined, though their thread-local destructors
/// may still run. A panic in `f` terminates the process.
pub fn scope<'env, F, T>(f: F) -> T
where
    F: for<'scope> FnOnce(&'scope Scope<'scope, 'env>) -> T,
{
    let scope = Scope {
        data: Arc::new(ScopeData {
            running: AtomicU32::new(0),
        }),
        lifetimes: PhantomData,
    };
    // The threads may borrow from the caller's frames, which an unwind out
    // of `f` would free while they run: abort instead.
    let result = abort_on_unwind(|| f(&scope));
    scope.data.wait();
    result
}

impl<'scope> Scope<'scope, '_> {
    /// Spawns a new thread within a scope, returning a [`ScopedJoinHandle`]
    /// for it. The thread is joined at the end of the scope if the handle is
    /// dropped.
    ///
    /// # Panics
    ///
    /// Panics if the OS fails to create a thread; use
    /// [`Builder::spawn_scoped`] to handle the error.
    #[track_caller]
    pub fn spawn<F, T>(&'scope self, f: F) -> ScopedJoinHandle<'scope, T>
    where
        F: FnOnce() -> T + Send + 'scope,
        T: Send + 'scope,
    {
        let Ok(handle) = Builder::new().spawn_scoped(self, f) else {
            spawn_failed()
        };
        handle
    }
}

impl Builder {
    /// Spawns a new scoped thread using the settings set through this
    /// `Builder`.
    ///
    /// # Errors
    ///
    /// Returns the OS error if the thread cannot be created.
    pub fn spawn_scoped<'scope, 'env, F, T>(
        self,
        scope: &'scope Scope<'scope, 'env>,
        f: F,
    ) -> io::Result<ScopedJoinHandle<'scope, T>>
    where
        F: FnOnce() -> T + Send + 'scope,
        T: Send + 'scope,
    {
        // SAFETY: `scope` waits for the thread before `'scope` ends, and
        // everything `F` and `T` borrow outlives `'scope`.
        Ok(ScopedJoinHandle(unsafe {
            spawn::spawn(self, Some(&scope.data), f)
        }?))
    }
}

impl<T> ScopedJoinHandle<'_, T> {
    /// Extracts a handle to the underlying thread.
    #[must_use]
    // std's `thread` is not `const`.
    #[allow(clippy::missing_const_for_fn)]
    pub fn thread(&self) -> &Thread {
        self.0.thread()
    }

    /// Waits for the associated thread to finish, including its
    /// thread-local destructors. Everything the thread did happens before
    /// `join` returns.
    ///
    /// # Errors
    ///
    /// Never fails: a panicking thread aborts the process.
    #[inline]
    pub fn join(self) -> Result<T> {
        // Visibly `Ok`, so that callers' `unwrap` compiles to nothing.
        Ok(self.0.join())
    }

    /// Checks if the associated thread has finished running its main
    /// function, without blocking; [`join`](Self::join) then returns soon.
    // std's `is_finished` is not `#[must_use]`.
    #[allow(clippy::must_use_candidate)]
    pub fn is_finished(&self) -> bool {
        self.0.is_finished()
    }
}

impl fmt::Debug for Scope<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scope")
            .field(
                "num_running_threads",
                &(self.data.running.load(Relaxed) & !SLEEPING),
            )
            .finish_non_exhaustive()
    }
}

impl<T> fmt::Debug for ScopedJoinHandle<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScopedJoinHandle").finish_non_exhaustive()
    }
}
