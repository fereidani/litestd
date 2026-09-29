//! The code that `Mutex`, `RwLock` and their guards share. Names come from
//! the invoking module, which imports `core::fmt` and `LockResult`.

/// Implements `Debug` and `Display` for lock guards, which show the data they
/// protect.
macro_rules! impl_guard_fmt {
    ($($guard:ident),*) => {$(
        impl<T: ?Sized + fmt::Debug> fmt::Debug for $guard<'_, T> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                (**self).fmt(f)
            }
        }

        impl<T: ?Sized + fmt::Display> fmt::Display for $guard<'_, T> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                (**self).fmt(f)
            }
        }
    )*};
}

/// Implements the rest of a lock's API: the poison methods, which do nothing
/// because nothing is ever poisoned, `into_inner`, `get_mut`, `From` and
/// `Default`. The lock has a `new` and a `data` field.
macro_rules! impl_lock_api {
    ($lock:ident) => {
        impl<T: ?Sized> $lock<T> {
            /// Determines whether the lock is poisoned: always `false` in
            /// litestd.
            #[inline]
            #[allow(clippy::unused_self, clippy::missing_const_for_fn)]
            pub fn is_poisoned(&self) -> bool {
                false
            }

            /// Clears the poisoned state from a lock; does nothing in
            /// litestd.
            #[inline]
            #[allow(clippy::unused_self, clippy::missing_const_for_fn)]
            pub fn clear_poison(&self) {}

            /// Consumes this lock, returning the underlying data.
            ///
            /// # Errors
            ///
            /// Never fails: litestd locks are never poisoned.
            #[inline]
            pub fn into_inner(self) -> LockResult<T>
            where
                T: Sized,
            {
                Ok(self.data.into_inner())
            }

            /// Returns a mutable reference to the underlying data, without
            /// locking.
            ///
            /// # Errors
            ///
            /// Never fails: litestd locks are never poisoned.
            #[inline]
            // Not `const`, as in std.
            #[allow(clippy::missing_const_for_fn)]
            pub fn get_mut(&mut self) -> LockResult<&mut T> {
                Ok(self.data.get_mut())
            }
        }

        impl<T> From<T> for $lock<T> {
            /// Creates a new lock in an unlocked state, like `new`.
            fn from(t: T) -> Self {
                Self::new(t)
            }
        }

        impl<T: Default> Default for $lock<T> {
            /// Creates a new lock, with the `Default` value for `T`.
            fn default() -> Self {
                Self::new(T::default())
            }
        }
    };
}
