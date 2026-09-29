//! OS thread-local keys created on first use.

use core::sync::atomic::{
    AtomicUsize,
    Ordering::{Acquire, Release},
};

use crate::sys::tls;

/// A `sys::tls` key that is created when first needed, without blocking:
/// racing threads each make a key, the first to publish wins, and the
/// others free theirs.
pub(super) struct LazyKey {
    /// The key plus one, or zero before it exists.
    key: AtomicUsize,
    /// Makes a new key.
    create: fn() -> Option<usize>,
}

impl LazyKey {
    pub(super) const fn new(create: fn() -> Option<usize>) -> Self {
        Self {
            key: AtomicUsize::new(0),
            create,
        }
    }

    /// Returns the key if it was created already.
    #[inline]
    pub(super) fn get(&self) -> Option<usize> {
        // `Acquire` pairs with the `Release` that published the key, so the
        // C library's bookkeeping for the key is visible before its use.
        self.key.load(Acquire).checked_sub(1)
    }

    /// Returns the key, creating it first if needed. Returns `None` only if
    /// the process ran out of keys.
    #[inline]
    pub(super) fn force(&self) -> Option<usize> {
        self.get().or_else(|| self.init())
    }

    #[cold]
    fn init(&self) -> Option<usize> {
        let new = (self.create)()?;
        let Some(stored) = new.checked_add(1) else {
            // Keys are small integers; this cannot happen, but a key that
            // cannot be stored must not leak.
            // SAFETY: no thread has seen `new`, so none has a value in it.
            unsafe { tls::destroy(new) };
            return None;
        };
        // `Release` publishes the new key with its C library state;
        // `Acquire` on failure pairs with the winner's `Release`.
        match self.key.compare_exchange(0, stored, Release, Acquire) {
            Ok(_) => Some(new),
            Err(winner) => {
                // Another thread published its key first; free ours.
                // SAFETY: no thread has seen `new`, so none has a value in it.
                unsafe { tls::destroy(new) };
                winner.checked_sub(1)
            }
        }
    }
}
