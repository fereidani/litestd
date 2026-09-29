//! [`Mutex`] and [`MutexGuard`].

use core::{
    cell::UnsafeCell,
    fmt,
    marker::PhantomData,
    ops::{Deref, DerefMut},
    panic::{RefUnwindSafe, UnwindSafe},
};

use super::{
    poison::{LockResult, TryLockError, TryLockResult},
    raw_mutex::RawMutex,
};

/// A mutual exclusion primitive useful for protecting shared data.
///
/// Uncontended locking and unlocking are one atomic operation each; a
/// contended lock spins briefly, then sleeps on a futex. It is never
/// poisoned: [`lock`] always returns `Ok` and [`is_poisoned`] `false`.
///
/// # Examples
///
/// ```
/// use litestd::sync::Mutex;
///
/// static COUNTER: Mutex<u32> = Mutex::new(0);
///
/// *COUNTER.lock().unwrap() += 1;
/// assert_eq!(*COUNTER.lock().unwrap(), 1);
/// ```
///
/// [`lock`]: Self::lock
/// [`is_poisoned`]: Self::is_poisoned
pub struct Mutex<T: ?Sized> {
    raw: RawMutex,
    data: UnsafeCell<T>,
}

// `Send` follows from the fields: `Mutex<T>` is `Send` if `T` is.

// SAFETY: the mutex gives one thread at a time access to `T`, so sharing it
// needs `T: Send` but not `T: Sync`.
unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}

// As in std, a mutex is unwind safe whatever it holds.
impl<T: ?Sized> UnwindSafe for Mutex<T> {}
impl<T: ?Sized> RefUnwindSafe for Mutex<T> {}

/// An RAII implementation of a "scoped lock" of a mutex. When this structure
/// is dropped (falls out of scope), the lock will be unlocked.
///
/// Created by [`Mutex::lock`] and [`Mutex::try_lock`]. As in std, a guard
/// cannot be sent to another thread:
///
/// ```compile_fail,E0277
/// use litestd::sync::Mutex;
///
/// fn assert_send<T: Send>(_: &T) {}
///
/// let mutex = Mutex::new(0);
/// let guard = mutex.lock().unwrap();
/// assert_send(&guard);
/// ```
///
/// A guard is invariant over the data type, so it cannot be used to store a
/// shorter-lived reference:
///
/// ```compile_fail
/// use litestd::sync::MutexGuard;
///
/// fn shorten<'a>(
///     guard: MutexGuard<'a, &'static str>,
/// ) -> MutexGuard<'a, &'a str> {
///     guard
/// }
/// ```
#[must_use = "if unused the Mutex will immediately unlock"]
#[clippy::has_significant_drop]
pub struct MutexGuard<'a, T: ?Sized> {
    // A reference to the whole mutex keeps the guard invariant over `T`, as
    // `DerefMut` requires.
    lock: &'a Mutex<T>,
    // Makes the guard `!Send` (and `!Sync`, restored below): std guards
    // must be released by the thread that acquired them.
    _not_send: PhantomData<*const ()>,
}

// SAFETY: sharing the guard only shares `&T`, which is sound when `T: Sync`.
unsafe impl<T: ?Sized + Sync> Sync for MutexGuard<'_, T> {}

impl<T> Mutex<T> {
    /// Creates a new mutex in an unlocked state ready for use.
    #[inline]
    pub const fn new(t: T) -> Self {
        Self {
            raw: RawMutex::new(),
            data: UnsafeCell::new(t),
        }
    }
}

impl<T: ?Sized> Mutex<T> {
    /// Acquires a mutex, blocking the current thread until it is able to do
    /// so. Locking it again on the same thread deadlocks.
    ///
    /// # Errors
    ///
    /// Never fails: litestd mutexes are never poisoned.
    #[inline]
    pub fn lock(&self) -> LockResult<MutexGuard<'_, T>> {
        self.raw.lock();
        // SAFETY: the lock was just acquired.
        Ok(unsafe { MutexGuard::new(self) })
    }

    /// Attempts to acquire this lock without blocking.
    ///
    /// # Errors
    ///
    /// Returns [`TryLockError::WouldBlock`] if the mutex is already locked.
    #[inline]
    pub fn try_lock(&self) -> TryLockResult<MutexGuard<'_, T>> {
        if self.raw.try_lock() {
            // SAFETY: the lock was just acquired.
            Ok(unsafe { MutexGuard::new(self) })
        } else {
            Err(TryLockError::WouldBlock)
        }
    }
}

impl_lock_api!(Mutex);

impl<T: ?Sized + fmt::Debug> fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("Mutex");
        // The guard lives until the end of its match arm, so the data stays
        // locked while it is formatted.
        match self.try_lock() {
            Ok(guard) => d.field("data", &&*guard),
            // std prints a quoted string here.
            Err(_) => d.field("data", &"<locked>"),
        };
        d.field("poisoned", &false);
        d.finish_non_exhaustive()
    }
}

impl<'a, T: ?Sized> MutexGuard<'a, T> {
    /// Wraps a locked mutex.
    ///
    /// # Safety
    ///
    /// The caller must hold the lock of `lock` and hand its release over to
    /// the guard.
    #[inline]
    const unsafe fn new(lock: &'a Mutex<T>) -> Self {
        Self {
            lock,
            _not_send: PhantomData,
        }
    }

    /// Returns the guard's raw lock, for [`Condvar`](super::Condvar); an
    /// associated function, so that it cannot shadow a method of `T`.
    #[inline]
    pub(super) const fn raw(guard: &Self) -> &'a RawMutex {
        &guard.lock.raw
    }
}

impl<T: ?Sized> Deref for MutexGuard<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: the guard holds the lock, so no other thread accesses the
        // data, and the `&self` borrow prevents a `&mut T` from this guard.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for MutexGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the guard holds the lock, so no other thread accesses the
        // data, and the `&mut self` borrow makes this reference unique.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for MutexGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: the guard holds the lock and releases it exactly once,
        // here; no reference derived from the guard outlives it.
        unsafe { self.lock.raw.unlock() };
    }
}

impl_guard_fmt!(MutexGuard);
