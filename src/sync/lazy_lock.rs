//! [`LazyLock`].

use core::{
    cell::UnsafeCell,
    fmt,
    mem::ManuallyDrop,
    ops::{Deref, DerefMut},
    panic::{RefUnwindSafe, UnwindSafe},
};

use super::once::Once;
use crate::unwind::abort_on_unwind;

/// The initializer before the value exists, and the value after. The state
/// of the `Once` tells which: `f` until the initialization is claimed,
/// `value` once it completes. A claim always completes, because an
/// unwinding initializer aborts the process.
union Data<T, F> {
    value: ManuallyDrop<T>,
    f: ManuallyDrop<F>,
}

/// A value which is initialized on the first access.
///
/// A thread-safe [`LazyCell`] that can be used in statics. Dereferencing
/// blocks while another thread initializes it, and reading an initialized
/// value is a single atomic load. It is never poisoned: a panicking
/// initializer terminates the process.
///
/// [`LazyCell`]: core::cell::LazyCell
pub struct LazyLock<T, F = fn() -> T> {
    once: Once,
    data: UnsafeCell<Data<T, F>>,
}

// `Send` follows from the fields.

// SAFETY: sharing the lock shares `&T` across threads, which needs
// `T: Sync`, and lets any thread run `F` and store the `T` that the owner
// later drops, which needs `F: Send` and `T: Send`. No `&F` is created from
// a shared `LazyLock`, so `F: Sync` is not needed. The `Once` gives the
// initializer sole access to `data` until it completes.
unsafe impl<T: Sync + Send, F: Send> Sync for LazyLock<T, F> {}

impl<T: RefUnwindSafe + UnwindSafe, F: UnwindSafe> RefUnwindSafe
    for LazyLock<T, F>
{
}
impl<T: UnwindSafe, F: UnwindSafe> UnwindSafe for LazyLock<T, F> {}

impl<T, F: FnOnce() -> T> LazyLock<T, F> {
    /// Creates a new lazy value with the given initializing function.
    #[inline]
    pub const fn new(f: F) -> Self {
        Self {
            once: Once::new(),
            data: UnsafeCell::new(Data {
                f: ManuallyDrop::new(f),
            }),
        }
    }

    /// Forces the evaluation of this lazy value and returns a mutable
    /// reference to the result. A panic in the initializer terminates the
    /// process.
    #[inline]
    pub fn force_mut(this: &mut Self) -> &mut T {
        if !this.once.is_completed_mut() {
            // SAFETY: the `Once` is incomplete.
            unsafe { Self::initialize_mut(this) };
        }
        // SAFETY: the `Once` is complete now, so `value` is initialized, and
        // `&mut` makes the reference unique.
        unsafe { &mut this.data.get_mut().value }
    }

    /// Runs the initializer through exclusive access.
    ///
    /// # Safety
    ///
    /// The `Once` must be incomplete, so that `f` is still stored.
    #[cold]
    unsafe fn initialize_mut(this: &mut Self) {
        // SAFETY: the caller guarantees that `f` is still stored. It is taken
        // once: `value` and completion follow, and an unwinding `f`, which
        // would leave `f` for `Drop` to drop again, aborts instead.
        let f = unsafe { ManuallyDrop::take(&mut this.data.get_mut().f) };
        let value = abort_on_unwind(f);
        this.data.get_mut().value = ManuallyDrop::new(value);
        this.once.complete_mut();
    }

    /// Forces the evaluation of this lazy value and returns a reference to
    /// the result, blocking while another thread initializes it. A panic in
    /// the initializer terminates the process.
    #[inline]
    pub fn force(this: &Self) -> &T {
        if !this.once.is_completed() {
            this.initialize();
        }
        // SAFETY: the `Once` is complete now, so `value` is initialized and
        // never written again while shared.
        unsafe { &(*this.data.get()).value }
    }

    #[cold]
    fn initialize(&self) {
        let Some(init) = self.once.claim() else {
            return;
        };
        let data = self.data.get();
        // SAFETY: the claim gives this thread sole access to `data` until
        // `complete`, and the `Once` was incomplete, so `f` is still there.
        // An unwinding `f` would leave the claim open with `f` taken, so
        // `abort_on_unwind` aborts instead.
        let f = unsafe { ManuallyDrop::take(&mut (*data).f) };
        let value = abort_on_unwind(f);
        // SAFETY: as above, this thread still has sole access to `data`.
        unsafe { (*data).value = ManuallyDrop::new(value) };
        init.complete();
    }
}

impl<T, F> LazyLock<T, F> {
    /// Returns a mutable reference to the value if initialized, or `None`.
    #[inline]
    pub fn get_mut(this: &mut Self) -> Option<&mut T> {
        if this.once.is_completed_mut() {
            // SAFETY: the `Once` is complete, so `value` is initialized, and
            // `&mut` makes the reference unique.
            Some(unsafe { &mut this.data.get_mut().value })
        } else {
            None
        }
    }

    /// Returns a reference to the value if initialized, or `None`, without
    /// blocking.
    #[inline]
    pub fn get(this: &Self) -> Option<&T> {
        if this.once.is_completed() {
            // SAFETY: the `Once` is complete, so `value` is initialized and
            // never written again while shared.
            Some(unsafe { &(*this.data.get()).value })
        } else {
            None
        }
    }
}

impl<T, F> Drop for LazyLock<T, F> {
    fn drop(&mut self) {
        if self.once.is_completed_mut() {
            // SAFETY: a complete `Once` means `value` is initialized, and it
            // is dropped once, with the lock.
            unsafe { ManuallyDrop::drop(&mut self.data.get_mut().value) };
        } else {
            // SAFETY: with exclusive access no claim is open, so an
            // incomplete `Once` was never claimed and `f` is still there.
            unsafe { ManuallyDrop::drop(&mut self.data.get_mut().f) };
        }
    }
}

impl<T, F: FnOnce() -> T> Deref for LazyLock<T, F> {
    type Target = T;

    /// Dereferences the value, blocking while another thread initializes it.
    #[inline]
    fn deref(&self) -> &T {
        Self::force(self)
    }
}

impl<T, F: FnOnce() -> T> DerefMut for LazyLock<T, F> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        Self::force_mut(self)
    }
}

impl<T: Default> Default for LazyLock<T> {
    /// Creates a new lazy value using `Default` as the initializing function.
    #[inline]
    fn default() -> Self {
        Self::new(T::default)
    }
}

impl<T: fmt::Debug, F> fmt::Debug for LazyLock<T, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_tuple("LazyLock");
        match Self::get(self) {
            Some(v) => d.field(v),
            None => d.field(&format_args!("<uninit>")),
        };
        d.finish()
    }
}

impl<T, F> From<T> for LazyLock<T, F> {
    /// Constructs a `LazyLock` that starts already initialized with `value`.
    #[inline]
    fn from(value: T) -> Self {
        Self {
            once: Once::new_complete(),
            data: UnsafeCell::new(Data {
                value: ManuallyDrop::new(value),
            }),
        }
    }
}
