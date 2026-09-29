//! [`OnceLock`].

use core::{
    cell::UnsafeCell,
    fmt,
    panic::{RefUnwindSafe, UnwindSafe},
};

use super::once::Once;
use crate::unwind::abort_on_unwind;

/// A synchronization primitive which can nominally be written to only once.
///
/// A thread-safe cell that can be used in statics; reading an initialized
/// cell is a single atomic load.
///
/// As in std, a value whose destructor could observe a dangling borrow must
/// outlive the cell:
///
/// ```compile_fail,E0597
/// use litestd::sync::OnceLock;
///
/// struct Observer<'a>(&'a str);
///
/// impl Drop for Observer<'_> {
///     fn drop(&mut self) {}
/// }
///
/// let cell = OnceLock::new();
/// {
///     let s = String::from("gone");
///     let _ = cell.set(Observer(&s));
/// }
/// ```
///
/// Const code cannot drop a cell:
///
/// ```compile_fail,E0493
/// use litestd::sync::OnceLock;
///
/// const fn make_and_drop() {
///     let _cell = OnceLock::<u32>::new();
/// }
/// ```
///
/// The cell is invariant over `T`, so it cannot be used to store a
/// shorter-lived reference:
///
/// ```compile_fail
/// use litestd::sync::OnceLock;
///
/// fn shorten<'a>(cell: OnceLock<&'static str>) -> OnceLock<&'a str> {
///     cell
/// }
/// ```
pub struct OnceLock<T> {
    once: Once,
    // `Some` exactly when `once` is complete, except while the claiming
    // thread stores the value and then completes it. No `Drop` impl: the
    // `Option`'s drop glue drops the value, so the drop check sees exactly
    // what is dropped and, as with std's `#[may_dangle]`, lets a `T` without
    // a destructor, such as a reference, dangle when the cell is dropped.
    value: UnsafeCell<Option<T>>,
    // Gives the cell a destructor whatever `T` is, as std's `Drop` impl
    // does, so that const code cannot drop a cell that std would reject.
    // The marker is not generic, so it adds nothing to the drop check.
    _destructor: Destructor,
}

/// A destructor that does nothing; see the `OnceLock` field that holds it.
struct Destructor;

impl Drop for Destructor {
    #[inline]
    fn drop(&mut self) {}
}

// `Send` follows from the fields: `OnceLock<T>` is `Send` if `T` is.

// SAFETY: sharing the cell shares `&T` across threads, which needs
// `T: Sync`, and lets any thread store the value that the owner later drops
// or takes, which needs `T: Send`. The `Once` orders the single write before
// every read.
unsafe impl<T: Sync + Send> Sync for OnceLock<T> {}

impl<T: RefUnwindSafe + UnwindSafe> RefUnwindSafe for OnceLock<T> {}
impl<T: UnwindSafe> UnwindSafe for OnceLock<T> {}

impl<T> OnceLock<T> {
    /// Creates a new uninitialized cell.
    #[inline]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            once: Once::new(),
            value: UnsafeCell::new(None),
            _destructor: Destructor,
        }
    }

    /// Gets the reference to the underlying value, or `None` if the cell is
    /// uninitialized or being initialized. Never blocks.
    #[inline]
    pub fn get(&self) -> Option<&T> {
        if self.once.is_completed() {
            // SAFETY: `is_completed` observed completion with an `Acquire`
            // load.
            Some(unsafe { self.get_unchecked() })
        } else {
            None
        }
    }

    /// Gets the mutable reference to the underlying value, or `None` if the
    /// cell is uninitialized. Never blocks.
    #[inline]
    pub fn get_mut(&mut self) -> Option<&mut T> {
        // With exclusive access no initialization runs, so the slot holds a
        // value exactly when the `Once` is complete.
        debug_assert_eq!(
            self.value.get_mut().is_some(),
            self.once.is_completed_mut()
        );
        self.value.get_mut().as_mut()
    }

    /// Blocks the current thread until the cell is initialized.
    #[inline]
    pub fn wait(&self) -> &T {
        self.once.wait();
        // SAFETY: `wait` returns once the `Once` is complete.
        unsafe { self.get_unchecked() }
    }

    /// Initializes the contents of the cell to `value`, blocking while
    /// another thread initializes it.
    ///
    /// # Errors
    ///
    /// Returns `Err(value)` if the cell was already initialized.
    #[inline]
    pub fn set(&self, value: T) -> Result<(), T> {
        if self.once.is_completed() {
            return Err(value);
        }
        self.set_slow(value)
    }

    #[cold]
    fn set_slow(&self, value: T) -> Result<(), T> {
        match self.once.claim() {
            None => Err(value),
            Some(init) => {
                // SAFETY: the claim gives this thread sole access to the
                // slot until `complete`, and the slot holds `None`.
                unsafe { self.value.get().write(Some(value)) };
                init.complete();
                Ok(())
            }
        }
    }

    /// Gets the contents of the cell, initializing it to `f()` if the cell
    /// was uninitialized. Only one `f` runs, and other callers block
    /// meanwhile; reentrant initialization deadlocks, and a panic in `f`
    /// terminates the process.
    #[inline]
    pub fn get_or_init<F>(&self, f: F) -> &T
    where
        F: FnOnce() -> T,
    {
        if !self.once.is_completed() {
            self.initialize(f);
        }
        // SAFETY: either the `Once` was complete, or `initialize` returned,
        // which it only does once the `Once` is complete.
        unsafe { self.get_unchecked() }
    }

    #[cold]
    fn initialize<F: FnOnce() -> T>(&self, f: F) {
        if let Some(init) = self.once.claim() {
            // An unwinding `f` would leave the claim open forever, and every
            // other caller blocked; abort instead.
            let value = abort_on_unwind(f);
            // SAFETY: the claim gives this thread sole access to the slot
            // until `complete`, and the slot holds `None`.
            unsafe { self.value.get().write(Some(value)) };
            init.complete();
        }
    }

    /// Consumes the `OnceLock`, returning the wrapped value, or `None` if the
    /// cell was uninitialized.
    #[inline]
    pub fn into_inner(self) -> Option<T> {
        // Ownership rules out a running initialization, so the slot holds a
        // value exactly when the `Once` is complete.
        self.value.into_inner()
    }

    /// Takes the value out of this `OnceLock`, moving it back to an
    /// uninitialized state.
    #[inline]
    pub fn take(&mut self) -> Option<T> {
        // With exclusive access no initialization runs, so the slot holds a
        // value exactly when the `Once` is complete: empty both.
        debug_assert_eq!(
            self.value.get_mut().is_some(),
            self.once.is_completed_mut()
        );
        self.once = Once::new();
        self.value.get_mut().take()
    }

    /// Returns the value.
    ///
    /// # Safety
    ///
    /// The `Once` must be complete, and the caller must have observed that
    /// with an `Acquire` load (`is_completed`, `wait` or `claim`).
    #[inline]
    unsafe fn get_unchecked(&self) -> &T {
        debug_assert!(self.once.is_completed());
        // SAFETY: `Some` was stored before the `Once` completed, at
        // construction or before the claimant's `Release` completion, which
        // the caller's `Acquire` load observed. The slot is never written
        // again while shared, so the unchecked unwrap is sound and free.
        unsafe { (*self.value.get()).as_ref().unwrap_unchecked() }
    }
}

impl<T> Default for OnceLock<T> {
    /// Creates a new uninitialized cell.
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<T: fmt::Debug> fmt::Debug for OnceLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_tuple("OnceLock");
        match self.get() {
            Some(v) => d.field(v),
            None => d.field(&format_args!("<uninit>")),
        };
        d.finish()
    }
}

impl<T: Clone> Clone for OnceLock<T> {
    #[inline]
    fn clone(&self) -> Self {
        self.get().map_or_else(Self::new, |v| Self::from(v.clone()))
    }
}

impl<T> From<T> for OnceLock<T> {
    /// Creates a new cell with its contents set to `value`.
    #[inline]
    fn from(value: T) -> Self {
        Self {
            once: Once::new_complete(),
            value: UnsafeCell::new(Some(value)),
            _destructor: Destructor,
        }
    }
}

impl<T: PartialEq> PartialEq for OnceLock<T> {
    /// Two `OnceLock`s are equal if both are empty or hold equal values.
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.get() == other.get()
    }
}

impl<T: Eq> Eq for OnceLock<T> {}
