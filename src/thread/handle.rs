//! Thread handles.

#[cfg(not(feature = "nightly"))]
use core::cell::UnsafeCell;
use core::{
    cell::Cell,
    fmt,
    mem::ManuallyDrop,
    panic::{RefUnwindSafe, UnwindSafe},
    ptr::{self, NonNull},
    sync::atomic::{
        AtomicUsize,
        Ordering::{Acquire, Relaxed, Release},
    },
};

use alloc_crate::string::String;

#[cfg(not(feature = "nightly"))]
use super::local_table::Locals;
use super::{id::ThreadId, parker::Parker};
use crate::{
    alloc::{GlobalAlloc, Layout, System},
    sys::os,
};

/// A handle to a thread. Clones are cheap and refer to the same thread.
pub struct Thread {
    inner: NonNull<Inner>,
}

/// What the handles of one thread share. It comes from [`System`], not the
/// global allocator, so that a global allocator may call
/// [`current`](super::current()) and use thread-locals.
///
/// `refs` counts the handles, but the thread's own slot, while it holds a
/// handle, counts as `SLOT`, and the handles `current` makes from the slot
/// count in `unmerged` instead, without an atomic operation. Dropping any
/// handle decrements `refs`, which stays at least `SLOT - unmerged`, far
/// above zero, until the slot releases its handle and merges `unmerged`.
struct Inner {
    /// The number of handles, with the slot's handle as `SLOT` and without
    /// those counted in `unmerged`.
    refs: AtomicUsize,
    /// The handles made from the slot and not yet added to `refs`. Only the
    /// thread itself touches it.
    unmerged: Cell<usize>,
    id: ThreadId,
    name: Name,
    parker: Parker,
    /// The thread's thread-local values; see `local_table`. Only the thread
    /// itself touches them. With `nightly`, they are native thread-locals.
    #[cfg(not(feature = "nightly"))]
    locals: UnsafeCell<Locals>,
}

/// A thread's name.
pub(crate) enum Name {
    Unnamed,
    /// The process's main thread, named `"main"` as in std.
    Main,
    /// The name given to [`Builder::name`](super::Builder::name).
    Given(String),
}

/// The share of `refs` that a thread's slot holds; see [`Inner`]. It leaves
/// a quarter of the range for other handles before `clone` aborts.
const SLOT: usize = usize::MAX / 4 + 1;

/// The count at which `unmerged` is added to `refs`, far below `SLOT`; low
/// under Miri, so that its tests reach it quickly.
const MERGE_AT: usize = if cfg!(miri) { 1 << 4 } else { 1 << 16 };

// SAFETY: a `Thread` is a counted reference to an `Inner`. Clones and drops
// on any thread only touch the atomic count, and the last drop frees the
// `Inner` after an `Acquire` load. Only the named thread touches `locals`
// and `unmerged`, the fields that are not `Send + Sync`, and the last drop
// frees nothing of them but the empty slot table.
unsafe impl Send for Thread {}
// SAFETY: as above; `&Thread` only reads immutable fields and uses the
// atomic parker.
unsafe impl Sync for Thread {}

// No handle method touches the `UnsafeCell` behind `locals`, so an unwind
// cannot leave a handle inconsistent; std's handle is unwind safe too.
impl UnwindSafe for Thread {}
impl RefUnwindSafe for Thread {}

impl Thread {
    /// Creates the first handle of a thread.
    // Out of line, so that its aligned allocation is not copied into each
    // instance of the generic spawn code.
    #[inline(never)]
    // `System` aligns blocks as the layout asks, here for `Inner`.
    #[allow(clippy::cast_ptr_alignment)]
    pub(crate) fn new(id: ThreadId, name: Name) -> Self {
        let layout = Layout::new::<Inner>();
        // An empty `Locals` is all zero bytes, so the large `locals` field,
        // where it exists, needs no code of its own.
        // SAFETY: `Inner` is not zero-sized.
        let inner = unsafe { System.alloc_zeroed(layout) }.cast::<Inner>();
        if inner.is_null() {
            // std's handle aborts too: the allocation error handler may need
            // the current thread.
            os::abort();
        }
        // SAFETY: `inner` is a fresh, non-null allocation with the layout of
        // `Inner`, so it can take each field; `locals` is initialized.
        unsafe {
            (&raw mut (*inner).refs).write(AtomicUsize::new(1));
            (&raw mut (*inner).unmerged).write(Cell::new(0));
            (&raw mut (*inner).id).write(id);
            (&raw mut (*inner).name).write(name);
            (&raw mut (*inner).parker).write(Parker::new());
            Self {
                inner: NonNull::new_unchecked(inner),
            }
        }
    }

    const fn inner(&self) -> &Inner {
        // SAFETY: this handle owns a reference, which keeps `Inner` alive.
        unsafe { self.inner.as_ref() }
    }

    /// Atomically makes the handle's token available if it is not already;
    /// see [`park`](super::park). Unparking a thread that is not parked is a
    /// single atomic operation.
    #[inline]
    pub fn unpark(&self) {
        self.inner().parker.unpark();
    }

    /// Gets the thread's unique identifier.
    #[must_use]
    // std's `Thread::id` is not `const`.
    #[allow(clippy::missing_const_for_fn)]
    pub fn id(&self) -> ThreadId {
        self.inner().id
    }

    /// Gets the thread's name: the one given to
    /// [`Builder::name`](super::Builder::name), exactly, or `"main"` for the
    /// main thread.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        match &self.inner().name {
            Name::Unnamed => None,
            Name::Main => Some("main"),
            Name::Given(name) => Some(name),
        }
    }

    /// Returns the name given to [`Builder::name`](super::Builder::name),
    /// which is also given to the operating system.
    pub(crate) fn given_name(&self) -> Option<&str> {
        match &self.inner().name {
            Name::Given(name) => Some(name),
            Name::Unnamed | Name::Main => None,
        }
    }

    pub(crate) const fn parker(&self) -> &Parker {
        &self.inner().parker
    }

    /// Returns the thread-local state of the thread whose handle `raw` came
    /// from.
    ///
    /// # Safety
    ///
    /// `raw` must come from [`Thread::into_slot`] and still own a live
    /// reference; only the thread it names may access the state.
    #[cfg(not(feature = "nightly"))]
    pub(super) const unsafe fn locals(raw: NonNull<u8>) -> *mut Locals {
        let inner = raw.cast::<Inner>().as_ptr();
        // SAFETY: the caller guarantees that `raw` points to a live `Inner`;
        // no reference to the rest of it is created.
        unsafe { UnsafeCell::raw_get(&raw const (*inner).locals) }
    }

    /// Turns the handle into the pointer that the thread's own slot holds,
    /// whose reference counts as `SLOT`.
    pub(super) fn into_slot(self) -> NonNull<u8> {
        // Relaxed, as in `clone`: this handle keeps `Inner` alive.
        self.inner().refs.fetch_add(SLOT - 1, Relaxed);
        ManuallyDrop::new(self).inner.cast()
    }

    /// Borrows the handle behind a pointer from [`Thread::into_slot`]; the
    /// `ManuallyDrop` must never be dropped.
    ///
    /// # Safety
    ///
    /// `raw` must come from `into_slot`, and its reference must stay live
    /// while the borrow is used.
    pub(super) const unsafe fn borrow_slot(
        raw: NonNull<u8>,
    ) -> ManuallyDrop<Self> {
        ManuallyDrop::new(Self { inner: raw.cast() })
    }

    /// Makes a new handle from the pointer that the calling thread's slot
    /// holds, counting it in `unmerged` without an atomic operation.
    ///
    /// # Safety
    ///
    /// `raw` must be the pointer that the calling thread's slot holds.
    #[inline]
    pub(super) unsafe fn clone_slot(raw: NonNull<u8>) -> Self {
        // SAFETY: the caller passes the slot's pointer, which keeps `Inner`
        // alive, and the new handle takes one reference, counted below.
        let this = ManuallyDrop::into_inner(unsafe { Self::borrow_slot(raw) });
        // The slot is the calling thread's, the only one using `unmerged`.
        let unmerged = &this.inner().unmerged;
        if unmerged.get() == MERGE_AT - 1 {
            // Relaxed: `refs` stays above zero either way, see `Inner`.
            let old = this.inner().refs.fetch_add(MERGE_AT, Relaxed);
            // As in `clone`: leaked handles must not wrap the count around.
            if old > usize::MAX / 2 {
                os::abort();
            }
            unmerged.set(0);
        } else {
            unmerged.set(unmerged.get() + 1);
        }
        this
    }

    /// Releases the reference that the calling thread's slot held, which
    /// then no longer holds the pointer.
    ///
    /// # Safety
    ///
    /// `raw` must be the pointer that the calling thread's slot held, and
    /// this must be its only release.
    pub(super) unsafe fn release_slot(raw: NonNull<u8>) {
        // SAFETY: the caller hands over the slot's reference, which keeps
        // `Inner` alive until the release.
        let this = unsafe { Self::borrow_slot(raw) };
        // The slot's `SLOT` and the handles in `unmerged`, which this thread
        // counted, as one count.
        let n = SLOT - this.inner().unmerged.get();
        // SAFETY: the slot owned these references and gives them up.
        unsafe { Self::release(this.inner, n) };
    }

    /// Drops `n` references to `inner`, freeing it if they were the last.
    ///
    /// # Safety
    ///
    /// The caller must own the `n` references and give them up.
    #[inline]
    unsafe fn release(inner: NonNull<Inner>, n: usize) {
        // `Release` orders this owner's uses before the free; see `free`.
        // SAFETY: the caller's references keep `Inner` alive.
        if unsafe { inner.as_ref() }.refs.fetch_sub(n, Release) == n {
            // SAFETY: these were the last references.
            unsafe { Self::free(inner) };
        }
    }

    /// Frees `inner` after its last reference was dropped.
    ///
    /// # Safety
    ///
    /// No reference to `inner` may be left.
    #[cold]
    #[inline(never)]
    unsafe fn free(inner: NonNull<Inner>) {
        // The last owner's `Acquire` load of its own decrement makes every
        // other owner's uses visible to it, as in `release_ref` of `spawn`.
        // SAFETY: nothing else can reach `Inner` now, which `new` allocated
        // from `System` with this layout.
        unsafe {
            inner.as_ref().refs.load(Acquire);
            ptr::drop_in_place(inner.as_ptr());
            System.dealloc(inner.as_ptr().cast(), Layout::new::<Inner>());
        }
    }
}

impl Clone for Thread {
    #[inline]
    fn clone(&self) -> Self {
        // As in `Arc`: the existing reference keeps `Inner` alive, so no
        // ordering is needed.
        let old = self.inner().refs.fetch_add(1, Relaxed);
        // Leaked clones overflow the count only after `usize::MAX / 2`.
        if old > usize::MAX / 2 {
            os::abort();
        }
        Self { inner: self.inner }
    }
}

impl Drop for Thread {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: the handle owns one reference and gives it up.
        unsafe { Self::release(self.inner, 1) };
    }
}

impl fmt::Debug for Thread {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Thread")
            .field("id", &self.id())
            .field("name", &self.name())
            .finish_non_exhaustive()
    }
}
