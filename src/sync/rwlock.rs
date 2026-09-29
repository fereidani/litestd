//! [`RwLock`], [`RwLockReadGuard`] and [`RwLockWriteGuard`].

use core::{
    cell::UnsafeCell,
    fmt,
    marker::PhantomData,
    mem::ManuallyDrop,
    ops::{Deref, DerefMut},
    panic::{RefUnwindSafe, UnwindSafe},
    ptr::NonNull,
};

use super::{
    poison::{LockResult, TryLockError, TryLockResult},
    raw_rwlock::RawRwLock,
};

/// A reader-writer lock.
///
/// It prefers writers: while a writer waits, new readers wait behind it, so
/// readers cannot starve writers. It is never poisoned: [`read`] and
/// [`write`] always return `Ok`.
///
/// [`read`]: Self::read
/// [`write`]: Self::write
pub struct RwLock<T: ?Sized> {
    raw: RawRwLock,
    data: UnsafeCell<T>,
}

// `Send` follows from the fields: `RwLock<T>` is `Send` if `T` is.

// SAFETY: writers get `&mut T` on the calling thread, which requires
// `T: Send`, and readers share `&T` across threads, which requires
// `T: Sync`; the lock never hands out both kinds of access at once.
unsafe impl<T: ?Sized + Send + Sync> Sync for RwLock<T> {}

// As in std, a lock is unwind safe whatever it holds.
impl<T: ?Sized> UnwindSafe for RwLock<T> {}
impl<T: ?Sized> RefUnwindSafe for RwLock<T> {}

/// RAII structure used to release the shared read access of a lock when
/// dropped.
///
/// Created by [`RwLock::read`] and [`RwLock::try_read`]. As in std, a guard
/// cannot be sent to another thread:
///
/// ```compile_fail,E0277
/// use litestd::sync::RwLock;
///
/// fn assert_send<T: Send>(_: &T) {}
///
/// let lock = RwLock::new(0);
/// let guard = lock.read().unwrap();
/// assert_send(&guard);
/// ```
#[must_use = "if unused the RwLock will immediately unlock"]
#[clippy::has_significant_drop]
pub struct RwLockReadGuard<'a, T: ?Sized + 'a> {
    // A pointer rather than `&'a T`: the data is only immutable until the
    // guard drops, not for all of `'a`. `NonNull` is covariant over `T`
    // like `&T`, and makes the guard `!Send` and `!Sync`.
    data: NonNull<T>,
    raw: &'a RawRwLock,
    /// On WebAssembly without threads, std's guard borrows a `Cell`, so it
    /// is neither `UnwindSafe` nor `RefUnwindSafe`: this matches it.
    #[cfg(all(target_family = "wasm", not(target_feature = "atomics")))]
    unwind: PhantomData<&'a core::cell::Cell<()>>,
}

// SAFETY: sharing the guard only shares `&T`, which is sound when `T: Sync`.
unsafe impl<T: ?Sized + Sync> Sync for RwLockReadGuard<'_, T> {}

/// RAII structure used to release the exclusive write access of a lock when
/// dropped.
///
/// Created by [`RwLock::write`] and [`RwLock::try_write`]. As in std, a
/// guard cannot be sent to another thread:
///
/// ```compile_fail,E0277
/// use litestd::sync::RwLock;
///
/// fn assert_send<T: Send>(_: &T) {}
///
/// let lock = RwLock::new(0);
/// let guard = lock.write().unwrap();
/// assert_send(&guard);
/// ```
///
/// A guard is invariant over the data type, so it cannot be used to store a
/// shorter-lived reference:
///
/// ```compile_fail
/// use litestd::sync::RwLockWriteGuard;
///
/// fn shorten<'a>(
///     guard: RwLockWriteGuard<'a, &'static str>,
/// ) -> RwLockWriteGuard<'a, &'a str> {
///     guard
/// }
/// ```
#[must_use = "if unused the RwLock will immediately unlock"]
#[clippy::has_significant_drop]
pub struct RwLockWriteGuard<'a, T: ?Sized> {
    // A reference to the whole lock keeps the guard invariant over `T`, as
    // `DerefMut` requires.
    lock: &'a RwLock<T>,
    // Makes the guard `!Send` (and `!Sync`, restored below).
    _not_send: PhantomData<*const ()>,
}

// SAFETY: sharing the guard only shares `&T`, which is sound when `T: Sync`.
unsafe impl<T: ?Sized + Sync> Sync for RwLockWriteGuard<'_, T> {}

impl<T> RwLock<T> {
    /// Creates a new instance of an `RwLock<T>` which is unlocked.
    #[inline]
    pub const fn new(t: T) -> Self {
        Self {
            raw: RawRwLock::new(),
            data: UnsafeCell::new(t),
        }
    }
}

impl<T: ?Sized> RwLock<T> {
    /// Locks this `RwLock` with shared read access, blocking the current
    /// thread while a writer holds or waits for the lock.
    ///
    /// A read lock taken while the current thread holds the write lock
    /// deadlocks, and so may a second one while a writer waits. Beyond
    /// 2^30 - 2 readers, where std panics, a reader waits until every read
    /// lock is released.
    ///
    /// # Errors
    ///
    /// Never fails: litestd locks are never poisoned.
    #[inline]
    pub fn read(&self) -> LockResult<RwLockReadGuard<'_, T>> {
        self.raw.read();
        // SAFETY: a read lock was just acquired.
        Ok(unsafe { RwLockReadGuard::new(self) })
    }

    /// Attempts to acquire this `RwLock` with shared read access, without
    /// blocking.
    ///
    /// # Errors
    ///
    /// Returns [`TryLockError::WouldBlock`] if the lock is write-locked or a
    /// writer waits for it.
    #[inline]
    pub fn try_read(&self) -> TryLockResult<RwLockReadGuard<'_, T>> {
        if self.raw.try_read() {
            // SAFETY: a read lock was just acquired.
            Ok(unsafe { RwLockReadGuard::new(self) })
        } else {
            Err(TryLockError::WouldBlock)
        }
    }

    /// Locks this `RwLock` with exclusive write access, blocking the current
    /// thread until it can be acquired. Taking it while the current thread
    /// holds any lock on it deadlocks.
    ///
    /// # Errors
    ///
    /// Never fails: litestd locks are never poisoned.
    #[inline]
    pub fn write(&self) -> LockResult<RwLockWriteGuard<'_, T>> {
        self.raw.write();
        // SAFETY: the write lock was just acquired.
        Ok(unsafe { RwLockWriteGuard::new(self) })
    }

    /// Attempts to lock this `RwLock` with exclusive write access, without
    /// blocking.
    ///
    /// # Errors
    ///
    /// Returns [`TryLockError::WouldBlock`] if the lock is already locked.
    #[inline]
    pub fn try_write(&self) -> TryLockResult<RwLockWriteGuard<'_, T>> {
        if self.raw.try_write() {
            // SAFETY: the write lock was just acquired.
            Ok(unsafe { RwLockWriteGuard::new(self) })
        } else {
            Err(TryLockError::WouldBlock)
        }
    }
}

impl_lock_api!(RwLock);

impl<T: ?Sized + fmt::Debug> fmt::Debug for RwLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("RwLock");
        match self.try_read() {
            Ok(guard) => d.field("data", &&*guard),
            // std prints an unquoted placeholder here.
            Err(_) => d.field("data", &format_args!("<locked>")),
        };
        d.field("poisoned", &false);
        d.finish_non_exhaustive()
    }
}

impl<'a, T: ?Sized> RwLockReadGuard<'a, T> {
    /// Wraps a read-locked lock.
    ///
    /// # Safety
    ///
    /// The caller must hold a read lock of `lock` and hand its release over
    /// to the guard.
    #[inline]
    const unsafe fn new(lock: &'a RwLock<T>) -> Self {
        Self {
            // SAFETY: `UnsafeCell::get` derives the pointer from a
            // reference, so it is never null.
            data: unsafe { NonNull::new_unchecked(lock.data.get()) },
            raw: &lock.raw,
            #[cfg(all(
                target_family = "wasm",
                not(target_feature = "atomics")
            ))]
            unwind: PhantomData,
        }
    }
}

impl<'a, T: ?Sized> RwLockWriteGuard<'a, T> {
    /// Wraps a write-locked lock.
    ///
    /// # Safety
    ///
    /// The caller must hold the write lock of `lock` and hand its release
    /// over to the guard.
    #[inline]
    const unsafe fn new(lock: &'a RwLock<T>) -> Self {
        Self {
            lock,
            _not_send: PhantomData,
        }
    }

    /// Downgrades a write-locked `RwLockWriteGuard` into a read-locked
    /// [`RwLockReadGuard`] atomically, so that no writer gets in between.
    /// Waiting readers are let in, even ahead of waiting writers.
    pub fn downgrade(s: Self) -> RwLockReadGuard<'a, T> {
        // The write guard must not unlock: its lock becomes the read lock.
        let lock = ManuallyDrop::new(s).lock;
        // SAFETY: the guard held the write lock, and gave it up above.
        unsafe { lock.raw.downgrade() };
        // SAFETY: `downgrade` left this thread holding a read lock.
        unsafe { RwLockReadGuard::new(lock) }
    }
}

impl<T: ?Sized> Deref for RwLockReadGuard<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: the guard holds a read lock, so no thread writes the data
        // until it drops, and `data` points into the live lock.
        unsafe { self.data.as_ref() }
    }
}

impl<T: ?Sized> Deref for RwLockWriteGuard<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: the guard holds the write lock, so no other thread
        // accesses the data, and `&self` prevents a `&mut T` from the guard.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for RwLockWriteGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the guard holds the write lock, so no other thread
        // accesses the data, and `&mut self` makes this reference unique.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwLockReadGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: the guard holds a read lock and releases it exactly once,
        // here; no reference derived from the guard outlives it.
        unsafe { self.raw.read_unlock() };
    }
}

impl<T: ?Sized> Drop for RwLockWriteGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: the guard holds the write lock and releases it exactly
        // once, here; no reference derived from the guard outlives it.
        unsafe { self.lock.raw.write_unlock() };
    }
}

impl_guard_fmt!(RwLockReadGuard, RwLockWriteGuard);
