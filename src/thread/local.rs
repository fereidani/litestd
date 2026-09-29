//! Thread-local storage: [`LocalKey`] and the `thread_local!` macro.
//!
//! The storage that the macro expands to comes from `local_table`, which
//! finds every value through one OS key, or with the `nightly` feature from
//! `local_native`, which keeps each static in a native thread-local of its
//! own.

use core::{
    cell::{Cell, RefCell},
    error::Error,
    fmt,
    ptr::NonNull,
};

#[cfg(not(feature = "nightly"))]
use super::local_table::Storage;

/// A thread local storage (TLS) key which owns its contents.
///
/// Created with [`thread_local!`]; [`with`] lends a shared reference that
/// cannot escape the closure, so mutation needs a [`Cell`] or [`RefCell`].
/// A value is created on first use and destroyed when its thread exits:
/// newest first, before [`join`] returns. Destructors can use values not
/// destroyed yet; access during or after destruction fails with
/// [`AccessError`]. A panic in an initializer or destructor terminates the
/// process. Destructors do not run for the Unix main thread or for threads
/// that exit as fibers.
///
/// By default, all statics share one OS key, a pthread key on Unix and a
/// fiber-local storage index on Windows, where values thus belong to a
/// fiber; as in std, the process aborts if the OS cannot provide it. Each
/// value lives in a heap block from [`System`], so that a global allocator
/// may use thread-locals. An initializer that depends on itself leaks its
/// first value until the thread exits, and a deleted fiber's values leak
/// unless a non-fiber thread without thread-locals of its own deletes it.
///
/// With the `nightly` feature, each static is a native thread-local of its
/// own, as in std, so values belong to the OS thread, fibers included, and
/// need no allocation. As in std, a value whose type needs no drop is never
/// destroyed, and an initializer that depends on itself drops its first
/// value when it returns. Windows keeps values aligned to more than two
/// words in heap blocks, which the thread's exit frees. Values leak if the
/// thread exits from a fiber that never called [`current`] nor initialized
/// a value that needs drop. On macOS, dyld runs the destructors, for the
/// main thread too, at `exit`.
///
/// [`with`]: LocalKey::with
/// [`join`]: super::JoinHandle::join
/// [`thread_local!`]: crate::thread_local
/// [`System`]: crate::alloc::System
/// [`current`]: super::current()
pub struct LocalKey<T: 'static> {
    #[cfg(not(feature = "nightly"))]
    storage: fn() -> &'static Storage<T>,
    /// Returns a pointer to the calling thread's value, initialized first
    /// from the argument if that holds one and from the initializer
    /// otherwise, or null while or after the value is destroyed.
    #[cfg(feature = "nightly")]
    get: fn(Option<&mut Option<T>>) -> *const T,
}

impl<T: 'static> fmt::Debug for LocalKey<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalKey").finish_non_exhaustive()
    }
}

/// Panics because a thread-local value is being or was destroyed.
#[cold]
#[inline(never)]
#[track_caller]
#[allow(clippy::panic)]
fn panic_access_error() -> ! {
    // std's `LocalKey::with` panics here, and so do the helpers built on it,
    // with the `Debug` form of the `AccessError`.
    panic!(
        "cannot access a Thread Local Storage value during or after \
         destruction: AccessError"
    )
}

impl<T: 'static> LocalKey<T> {
    /// Not public API; use [`thread_local!`](crate::thread_local) instead.
    #[cfg(not(feature = "nightly"))]
    #[doc(hidden)]
    pub const fn new(storage: fn() -> &'static Storage<T>) -> Self {
        Self { storage }
    }

    /// Not public API; use [`thread_local!`](crate::thread_local) instead.
    ///
    /// # Safety
    ///
    /// `get` must return null or a pointer to the calling thread's value,
    /// which stays valid until the thread exits; only an initializer that
    /// `get` itself runs may replace the value.
    #[cfg(feature = "nightly")]
    #[doc(hidden)]
    pub const unsafe fn new(
        get: fn(Option<&mut Option<T>>) -> *const T,
    ) -> Self {
        Self { get }
    }

    /// Acquires a reference to the value in this TLS key, initializing it
    /// first if needed.
    ///
    /// # Panics
    ///
    /// Panics if the value is being or has been destroyed.
    pub fn with<F, R>(&'static self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        match self.try_with(f) {
            Ok(r) => r,
            Err(AccessError) => panic_access_error(),
        }
    }

    /// Acquires a reference to the value in this TLS key, initializing it
    /// first if needed.
    ///
    /// # Errors
    ///
    /// Returns [`AccessError`] if the value is being or has been destroyed.
    #[inline]
    pub fn try_with<F, R>(&'static self, f: F) -> Result<R, AccessError>
    where
        F: FnOnce(&T) -> R,
    {
        let value = self.value().ok_or(AccessError)?;
        // SAFETY: a value is dropped when its thread exits, after the
        // thread's own code is done, or when an initializer that depends on
        // it returns, which is not during `f`; so the value outlives `f`.
        Ok(f(unsafe { value.as_ref() }))
    }

    /// Returns the calling thread's value, initialized first if needed, or
    /// `None` while or after it is destroyed.
    #[cfg(not(feature = "nightly"))]
    #[inline]
    fn value(&'static self) -> Option<NonNull<T>> {
        let storage = (self.storage)();
        storage.get().or_else(|| storage.initialize(None))
    }

    /// Returns the calling thread's value, initialized first if needed, or
    /// `None` while or after it is destroyed.
    #[cfg(feature = "nightly")]
    #[inline]
    fn value(&'static self) -> Option<NonNull<T>> {
        NonNull::new((self.get)(None).cast_mut())
    }

    /// Acquires a reference to the value, initializing it with `init` if
    /// needed. `f` gets `init` back if the value was already initialized.
    #[cfg(not(feature = "nightly"))]
    fn initialize_with<F, R>(&'static self, init: T, f: F) -> R
    where
        F: FnOnce(Option<T>, &T) -> R,
    {
        let storage = (self.storage)();
        if let Some(value) = storage.get() {
            // SAFETY: as in `try_with`.
            return f(Some(init), unsafe { value.as_ref() });
        }
        // Only the slow path lends `init` out, so that the fast path keeps
        // it in registers.
        let mut init = Some(init);
        let Some(value) = storage.initialize(Some(&mut init)) else {
            panic_access_error()
        };
        // SAFETY: as in `try_with`.
        f(init, unsafe { value.as_ref() })
    }

    /// Acquires a reference to the value, initializing it with `init` if
    /// needed. `f` gets `init` back if the value was already initialized.
    #[cfg(feature = "nightly")]
    fn initialize_with<F, R>(&'static self, init: T, f: F) -> R
    where
        F: FnOnce(Option<T>, &T) -> R,
    {
        // Only the slow path of the storage takes `init` out.
        let mut init = Some(init);
        let Some(value) = NonNull::new((self.get)(Some(&mut init)).cast_mut())
        else {
            panic_access_error()
        };
        // SAFETY: as in `try_with`.
        f(init, unsafe { value.as_ref() })
    }
}

impl<T: 'static> LocalKey<Cell<T>> {
    /// Sets or initializes the contained value, without running the lazy
    /// initializer.
    ///
    /// # Panics
    ///
    /// Panics if the value is being or has been destroyed.
    pub fn set(&'static self, value: T) {
        self.initialize_with(Cell::new(value), |value, cell| {
            if let Some(value) = value {
                // The cell was initialized already; overwrite its value.
                cell.set(value.into_inner());
            }
        });
    }

    /// Returns a copy of the contained value.
    ///
    /// # Panics
    ///
    /// Panics if the value is being or has been destroyed.
    pub fn get(&'static self) -> T
    where
        T: Copy,
    {
        self.with(Cell::get)
    }

    /// Takes the contained value, leaving `Default::default()` in its place.
    ///
    /// # Panics
    ///
    /// Panics if the value is being or has been destroyed.
    pub fn take(&'static self) -> T
    where
        T: Default,
    {
        self.with(Cell::take)
    }

    /// Replaces the contained value, returning the old value.
    ///
    /// # Panics
    ///
    /// Panics if the value is being or has been destroyed.
    pub fn replace(&'static self, value: T) -> T {
        self.with(|cell| cell.replace(value))
    }

    /// Updates the contained value using a function.
    ///
    /// # Panics
    ///
    /// Panics if the value is being or has been destroyed.
    pub fn update(&'static self, f: impl FnOnce(T) -> T)
    where
        T: Copy,
    {
        self.with(|cell| cell.set(f(cell.get())));
    }
}

impl<T: 'static> LocalKey<RefCell<T>> {
    /// Acquires a reference to the contained value.
    ///
    /// # Panics
    ///
    /// Panics if the value is mutably borrowed, or is being or has been
    /// destroyed.
    pub fn with_borrow<F, R>(&'static self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        self.with(|cell| f(&cell.borrow()))
    }

    /// Acquires a mutable reference to the contained value.
    ///
    /// # Panics
    ///
    /// Panics if the value is borrowed, or is being or has been destroyed.
    pub fn with_borrow_mut<F, R>(&'static self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R,
    {
        self.with(|cell| f(&mut cell.borrow_mut()))
    }

    /// Sets or initializes the contained value, without running the lazy
    /// initializer.
    ///
    /// # Panics
    ///
    /// Panics if the value is borrowed, or is being or has been destroyed.
    pub fn set(&'static self, value: T) {
        self.initialize_with(RefCell::new(value), |value, cell| {
            if let Some(value) = value {
                // The cell was initialized already; overwrite its value.
                *cell.borrow_mut() = value.into_inner();
            }
        });
    }

    /// Takes the contained value, leaving `Default::default()` in its place.
    ///
    /// # Panics
    ///
    /// Panics if the value is borrowed, or is being or has been destroyed.
    pub fn take(&'static self) -> T
    where
        T: Default,
    {
        self.with(RefCell::take)
    }

    /// Replaces the contained value, returning the old value.
    ///
    /// # Panics
    ///
    /// Panics if the value is borrowed, or is being or has been destroyed.
    pub fn replace(&'static self, value: T) -> T {
        self.with(|cell| cell.replace(value))
    }
}

/// An error returned by [`LocalKey::try_with`].
#[non_exhaustive]
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct AccessError;

impl fmt::Debug for AccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccessError").finish()
    }
}

impl fmt::Display for AccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt("already destroyed", f)
    }
}

impl Error for AccessError {}

/// Declares a new thread local storage key of type [`LocalKey`].
///
/// Each static may have attributes and a visibility. A `const { ... }`
/// initializer must be a constant expression, as in std. By default it
/// costs the same as any other, since every value lives in a heap block
/// made on first use. With the `nightly` feature, it makes a value whose
/// type needs no drop a plain native thread-local, which no access checks.
///
/// # Examples
///
/// ```
/// use litestd::{
///     cell::{Cell, RefCell},
///     thread_local,
/// };
///
/// thread_local! {
///     static FOO: Cell<u32> = const { Cell::new(1) };
///     static NAMES: RefCell<Vec<String>> = RefCell::new(Vec::new());
/// }
///
/// FOO.set(2);
/// assert_eq!(FOO.get(), 2);
/// NAMES.with_borrow_mut(|names| names.push("main".into()));
/// assert_eq!(NAMES.with_borrow(Vec::len), 1);
/// ```
#[macro_export]
macro_rules! thread_local {
    // The end of a list of declarations.
    () => {};

    (
        $(#[$attr:meta])* $vis:vis static $name:ident: $t:ty
            = const $init:block; $($rest:tt)*
    ) => (
        $crate::__thread_local_inner!($(#[$attr])* $vis $name, $t, const $init);
        $crate::thread_local!($($rest)*);
    );

    (
        $(#[$attr:meta])* $vis:vis static $name:ident: $t:ty
            = const $init:block
    ) => (
        $crate::__thread_local_inner!($(#[$attr])* $vis $name, $t, const $init);
    );

    (
        $(#[$attr:meta])* $vis:vis static $name:ident: $t:ty
            = $init:expr; $($rest:tt)*
    ) => (
        $crate::__thread_local_inner!($(#[$attr])* $vis $name, $t, $init);
        $crate::thread_local!($($rest)*);
    );

    (
        $(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = $init:expr
    ) => (
        $crate::__thread_local_inner!($(#[$attr])* $vis $name, $t, $init);
    );
}

/// Expands one declaration of [`thread_local!`]. Not public API. The
/// expansion has no `unsafe`, so it works in crates that forbid it.
#[cfg(not(feature = "nightly"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __thread_local_inner {
    // Closures, not functions: a signature naming `$t` would reject the
    // elided lifetimes that std accepts in a `const` initializer.
    ($(#[$attr:meta])* $vis:vis $name:ident, $t:ty, const $init:block) => {
        $(#[$attr])* $vis const $name: $crate::thread::LocalKey<$t> = {
            const __LITESTD_INIT: $t = $init;
            $crate::thread::LocalKey::new(|| {
                static __LITESTD_STORAGE:
                    $crate::thread::local_impl::Storage<$t> =
                    $crate::thread::local_impl::Storage::new(|| __LITESTD_INIT);
                &__LITESTD_STORAGE
            })
        };
    };

    ($(#[$attr:meta])* $vis:vis $name:ident, $t:ty, $init:expr) => {
        $(#[$attr])* $vis const $name: $crate::thread::LocalKey<$t> = {
            fn __litestd_init() -> $t {
                $init
            }
            $crate::thread::LocalKey::new(|| {
                static __LITESTD_STORAGE:
                    $crate::thread::local_impl::Storage<$t> =
                    $crate::thread::local_impl::Storage::new(__litestd_init);
                &__LITESTD_STORAGE
            })
        };
    };
}

/// Expands one declaration of [`thread_local!`]. Not public API. Its
/// `unsafe` block holds no code of the caller's, and works in crates that
/// forbid `unsafe`.
#[cfg(feature = "nightly")]
#[doc(hidden)]
#[macro_export]
#[allow_internal_unstable(thread_local)]
#[allow_internal_unsafe]
macro_rules! __thread_local_inner {
    // The accessor of a value that a `Storage`, or where `inline` says no a
    // `Boxed`, holds; `$init` is its initializer, a function of this macro.
    (@storage $t:ty, $init:expr) => {
        if $crate::thread::local_impl::inline::<$t>() {
            |__litestd_provided| {
                #[thread_local]
                static __LITESTD_VALUE: $crate::thread::local_impl::Storage<$t> =
                    $crate::thread::local_impl::Storage::new();
                __LITESTD_VALUE.get(__litestd_provided, $init)
            }
        } else {
            |__litestd_provided| {
                #[thread_local]
                static __LITESTD_VALUE: $crate::thread::local_impl::Boxed<$t> =
                    $crate::thread::local_impl::Boxed::new();
                __LITESTD_VALUE.get(__litestd_provided, $init)
            }
        }
    };

    ($(#[$attr:meta])* $vis:vis $name:ident, $t:ty, const $init:block) => {
        $(#[$attr])* $vis const $name: $crate::thread::LocalKey<$t> = {
            const __LITESTD_INIT: $t = $init;
            // SAFETY: each accessor returns null or a pointer to the calling
            // thread's value in a `#[thread_local]` static of its own, which
            // only an initializer that the accessor runs may replace.
            unsafe {
                $crate::thread::LocalKey::new(const {
                    if !$crate::mem::needs_drop::<$t>()
                        && $crate::thread::local_impl::inline::<$t>()
                    {
                        |_| {
                            #[thread_local]
                            static __LITESTD_VALUE: $t = __LITESTD_INIT;
                            &raw const __LITESTD_VALUE
                        }
                    } else {
                        // A closure: a function signature would reject
                        // elided lifetimes in `$t`, which std accepts here.
                        $crate::__thread_local_inner!(
                            @storage $t, || __LITESTD_INIT
                        )
                    }
                })
            }
        };
    };

    ($(#[$attr:meta])* $vis:vis $name:ident, $t:ty, $init:expr) => {
        $(#[$attr])* $vis const $name: $crate::thread::LocalKey<$t> = {
            fn __litestd_init() -> $t {
                $init
            }
            // SAFETY: as above.
            unsafe {
                $crate::thread::LocalKey::new(const {
                    $crate::__thread_local_inner!(@storage $t, __litestd_init)
                })
            }
        };
    };
}
