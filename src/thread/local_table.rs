//! Thread-local storage on top of one OS key, without the `nightly`
//! feature.
//!
//! `current` keeps the thread's handle in the one key with an exit hook,
//! and the handle holds the thread's [`Locals`]: one slot per
//! `thread_local!` static, at an index shared by all threads and assigned
//! on first use, and a list of the thread's values, each in a heap block
//! from [`System`]. A slot is null before initialization, points to the
//! value while it lives, then holds `DESTROYING` while its destructor runs
//! and `DESTROYED` afterwards; the last two make `try_with` fail.

use core::{
    fmt,
    ptr::{self, NonNull},
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
};

use super::{Thread, current};
use crate::{
    alloc::{GlobalAlloc, Layout, System},
    sys::os,
    unwind::abort_on_unwind,
};

/// The slot value of a static whose value is being destroyed.
const DESTROYING: usize = 1;
/// The slot value of a static whose value was destroyed.
const DESTROYED: usize = 2;

/// The index of a static that has none yet. It lies beyond every table, so
/// looking it up finds no value.
const UNASSIGNED: usize = usize::MAX;

/// The number of slots that every thread has without a further allocation.
const INLINE: usize = 32;

/// The header of a thread-local value, the same for every value type.
#[repr(C)]
#[derive(Clone, Copy)]
struct Node {
    /// The next older value of the same thread, or null.
    next: *mut Self,
    /// The index of the slot that points to this value.
    index: usize,
    /// Drops and frees the [`Value`] that this node heads.
    drop: unsafe fn(*mut Self),
}

/// A thread-local value in its heap block.
#[repr(C)]
struct Value<T> {
    node: Node,
    value: T,
}

impl<T> Value<T> {
    /// Moves `value` into a new heap block for the slot `index`.
    // `System` aligns blocks as the layout asks, here for `Self`.
    #[allow(clippy::cast_ptr_alignment)]
    fn new(index: usize, value: T) -> *mut Self {
        let layout = Layout::new::<Self>();
        // SAFETY: a `Value` is never zero-sized: it starts with a `Node`.
        let block = unsafe { System.alloc(layout) }.cast::<Self>();
        if block.is_null() {
            // std aborts too: the allocation error handler may use
            // thread-locals itself.
            os::abort();
        }
        let node = Node {
            next: ptr::null_mut(),
            index,
            drop: drop_value::<T>,
        };
        // SAFETY: `block` is a fresh allocation with the layout of `Self`.
        unsafe { block.write(Self { node, value }) };
        block
    }
}

/// Drops and frees a [`Value<T>`].
///
/// # Safety
///
/// `node` must head a `Value<T>` from [`Value::new`] that nothing uses any
/// more.
unsafe fn drop_value<T>(node: *mut Node) {
    let value = node.cast::<Value<T>>();
    // SAFETY: the caller hands over the value, which `Value::new` allocated
    // from `System` with this layout.
    unsafe {
        ptr::drop_in_place(value);
        System.dealloc(value.cast(), Layout::new::<Value<T>>());
    }
}

/// The thread-local state of one thread, kept in its handle.
///
/// Only the thread touches it, and only through raw pointers, since
/// initializers and destructors that run in between may touch it too. It
/// has a cache line of its own, apart from what other threads write, and
/// all zero bytes are an empty `Locals`, which is how `Thread::new` makes
/// one.
#[repr(C, align(64))]
pub(super) struct Locals {
    /// The slots of the first `INLINE` indices.
    inline: [*mut Node; INLINE],
    /// The slots of the indices from `INLINE` on, from `System`, or null.
    more: *mut *mut Node,
    /// The number of slots in `more`.
    more_len: usize,
    /// The newest value of the thread, which heads a list of all of them.
    head: *mut Node,
}

impl Drop for Locals {
    fn drop(&mut self) {
        if !self.more.is_null() {
            let layout = table_layout(self.more_len);
            // SAFETY: `grow` allocated `more` from `System` with the layout
            // of a table of `more_len` slots.
            unsafe { System.dealloc(self.more.cast(), layout) };
        }
    }
}

/// Returns the layout of a table of `len` slots, as `grow` allocates it.
/// Aborts if it does not fit the address space, which no program reaches;
/// checking at every use keeps the layout's validity local.
fn table_layout(len: usize) -> Layout {
    let Ok(layout) = Layout::array::<*mut Node>(len) else {
        os::abort()
    };
    layout
}

/// Returns what the calling thread's slot `index` holds, or null if the
/// thread has no such slot yet.
#[inline]
fn slot_value(index: usize) -> *mut Node {
    let Some(handle) = current::get() else {
        return ptr::null_mut();
    };
    // SAFETY: `handle` is the calling thread's handle, whose `Locals` only
    // this thread touches, and nothing else runs meanwhile.
    unsafe {
        let locals = Thread::locals(handle);
        match (*locals).inline.get(index) {
            Some(&slot) => slot,
            None => overflow_value(locals, index),
        }
    }
}

/// Returns what the slot `index` beyond the inline ones holds, or null.
///
/// # Safety
///
/// `locals` must be the calling thread's, and `index` at least `INLINE`.
#[inline]
unsafe fn overflow_value(locals: *mut Locals, index: usize) -> *mut Node {
    let i = index - INLINE;
    // SAFETY: the caller passes the calling thread's `Locals`, whose `more`
    // holds `more_len` slots.
    unsafe {
        if i < (*locals).more_len {
            *(*locals).more.add(i)
        } else {
            ptr::null_mut()
        }
    }
}

/// Returns the calling thread's slot `index`, making room for it first.
/// The pointer is valid until code that may initialize thread-locals runs.
///
/// # Safety
///
/// `locals` must be the calling thread's.
unsafe fn slot(locals: *mut Locals, index: usize) -> *mut *mut Node {
    if index < INLINE {
        // SAFETY: the caller passes a live `Locals`, and `index` is in
        // bounds of `inline`.
        return unsafe {
            (&raw mut (*locals).inline).cast::<*mut Node>().add(index)
        };
    }
    let i = index - INLINE;
    // SAFETY: as above; `grow` makes `more` hold at least `i + 1` slots.
    unsafe {
        if i >= (*locals).more_len {
            grow(locals, i);
        }
        (*locals).more.add(i)
    }
}

/// Replaces `more` with a larger table that holds the slot `i`.
///
/// # Safety
///
/// `locals` must be the calling thread's, and `i` at least `more_len`.
#[cold]
// `System` aligns blocks as the layout asks, here for `*mut Node`.
#[allow(clippy::cast_ptr_alignment)]
unsafe fn grow(locals: *mut Locals, i: usize) {
    // SAFETY: the caller passes the calling thread's live `Locals`.
    let (old, old_len) = unsafe { ((*locals).more, (*locals).more_len) };
    let len = old_len
        .saturating_mul(2)
        .max(i.saturating_add(1))
        .max(INLINE);
    let layout = table_layout(len);
    // SAFETY: the layout is not zero-sized. Zeroed slots are null.
    let more = unsafe { System.alloc_zeroed(layout) }.cast::<*mut Node>();
    if more.is_null() {
        os::abort();
    }
    if !old.is_null() {
        let old_layout = table_layout(old_len);
        // SAFETY: `old` holds `old_len` slots from `System` with
        // `old_layout`, and the fresh `more` holds `len > i >= old_len`, so
        // the copy fits and the blocks do not overlap.
        unsafe {
            ptr::copy_nonoverlapping(old, more, old_len);
            System.dealloc(old.cast(), old_layout);
        }
    }
    // SAFETY: as above.
    unsafe {
        (*locals).more = more;
        (*locals).more_len = len;
    }
}

/// Points the calling thread's slot for `node` to it and pushes it on the
/// thread's list, which owns it from then on.
///
/// # Safety
///
/// `locals` must be the calling thread's, and `node` must head a live
/// `Value` of the thread that nothing else owns.
unsafe fn register(locals: *mut Locals, node: *mut Node) {
    // SAFETY: the caller passes the calling thread's `Locals` and a node
    // that it owns; only this thread touches its list, which holds only
    // nodes of live values, destroyed exactly once by the exit hook.
    unsafe {
        *slot(locals, (*node).index) = node;
        (*node).next = (*locals).head;
        (*locals).head = node;
    }
}

/// Destroys the calling thread's values, newest first, including values
/// that the destructors initialize meanwhile.
///
/// # Safety
///
/// `locals` must be the calling thread's, and the thread must be exiting:
/// the values are freed.
pub(super) unsafe fn run_destructors(locals: *mut Locals) {
    // Each round destroys the values listed so far; their destructors push
    // new values on the emptied head. A destroyed static cannot be
    // initialized again, so each static adds at most one value per call and
    // the loop ends.
    loop {
        // SAFETY: only this thread touches its list.
        let list =
            unsafe { (&raw mut (*locals).head).replace(ptr::null_mut()) };
        if list.is_null() {
            return;
        }
        // SAFETY: the list is ours now.
        unsafe { destroy_all(locals, list) };
    }
}

/// Destroys every value on the list starting at `node`.
///
/// # Safety
///
/// `locals` must be the calling thread's, and the caller must own the list.
unsafe fn destroy_all(locals: *mut Locals, mut node: *mut Node) {
    // One pass over the finite list: destructors that initialize values
    // push them on the list's emptied head, not on this chain.
    while !node.is_null() {
        // SAFETY: listed nodes are live; the header is read before the value
        // is freed.
        let Node { next, index, drop } = unsafe { node.read() };
        // SAFETY: the list owns the value, which nothing borrows while the
        // thread exits. The destructor may grow the table, so the slot is
        // looked up again afterwards.
        unsafe {
            *slot(locals, index) = ptr::without_provenance_mut(DESTROYING);
            drop(node);
            *slot(locals, index) = ptr::without_provenance_mut(DESTROYED);
        }
        node = next;
    }
}

/// The storage behind a `thread_local!` static. Not public API.
#[doc(hidden)]
pub struct Storage<T: 'static> {
    /// The index of the static's slot, or `UNASSIGNED` before it is used.
    index: AtomicUsize,
    init: fn() -> T,
}

/// The number of slot indices handed out.
static INDICES: AtomicUsize = AtomicUsize::new(0);

impl<T: 'static> Storage<T> {
    /// Creates the storage for a static initialized with `init`.
    #[doc(hidden)]
    pub const fn new(init: fn() -> T) -> Self {
        Self {
            index: AtomicUsize::new(UNASSIGNED),
            init,
        }
    }

    /// Returns the calling thread's value, if it has a live one.
    #[inline]
    pub(super) fn get(&'static self) -> Option<NonNull<T>> {
        // The index names a slot in each thread's own table and publishes
        // nothing else, so it needs no ordering.
        let slot = slot_value(self.index.load(Relaxed));
        if slot.addr() <= DESTROYED {
            return None;
        }
        // SAFETY: a slot above the sentinels points to the thread's live
        // `Value<T>` for this static.
        let value = unsafe { &raw mut (*slot.cast::<Value<T>>()).value };
        // SAFETY: a field of a live value is not null.
        Some(unsafe { NonNull::new_unchecked(value) })
    }

    /// Initializes the calling thread's value, which `get` did not find,
    /// from `provided` if that holds one and from the initializer otherwise.
    /// Returns `None` while or after the value is destroyed.
    #[cold]
    pub(super) fn initialize(
        &'static self,
        provided: Option<&mut Option<T>>,
    ) -> Option<NonNull<T>> {
        let index = assign_index(&self.index);
        let locals = current::locals();
        // `get` found no live value, so a non-null slot holds `DESTROYING`
        // or `DESTROYED`.
        // SAFETY: `locals` is the calling thread's.
        if !unsafe { *slot(locals, index) }.is_null() {
            return None;
        }
        // If the initializer initializes this static too, its value stays on
        // the list until the thread exits and the slot takes this one.
        let value = provided
            .and_then(Option::take)
            .unwrap_or_else(|| abort_on_unwind(self.init));
        let block = Value::new(index, value);
        // SAFETY: `block` is a fresh value of this thread, and the handle,
        // with its `Locals`, stays in place while the thread runs code.
        unsafe { register(locals, block.cast()) };
        // SAFETY: `Value::new` never returns null, so `block` points to the
        // value, which lives until the thread exits.
        Some(unsafe { NonNull::new_unchecked(&raw mut (*block).value) })
    }
}

/// Returns the slot index in `index`, handing out a new one first if there
/// is none. Racing threads each take one; the first to store it wins, and
/// the others' indices stay unused.
#[cold]
fn assign_index(index: &AtomicUsize) -> usize {
    let assigned = index.load(Relaxed);
    if assigned != UNASSIGNED {
        return assigned;
    }
    // Distinct values need only the read-modify-write, in any memory order.
    let new = INDICES.fetch_add(1, Relaxed);
    if new >= UNASSIGNED / 2 {
        // Each static takes one index per racing thread; the count cannot
        // get here without wrapping around first.
        os::abort();
    }
    match index.compare_exchange(UNASSIGNED, new, Relaxed, Relaxed) {
        Ok(_) => new,
        Err(winner) => winner,
    }
}

impl<T: 'static> fmt::Debug for Storage<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Storage").finish_non_exhaustive()
    }
}
