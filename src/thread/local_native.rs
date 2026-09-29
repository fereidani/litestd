//! Native thread-local storage, with the `nightly` feature.
//!
//! Each `thread_local!` static expands to a `#[thread_local]` static of its
//! own. A `const`-initialized value whose type needs no drop is that static
//! itself. Any other value sits in a [`Storage`] beside a state, or, where
//! [`inline`] says so, in a heap block that a [`Boxed`] points to. The
//! first initialization of a value that must be dropped or freed links its
//! storage's [`Node`] into a list of the thread's own, whose head is a
//! native thread-local too, so that the list belongs to the OS thread even
//! where the thread's handle belongs to a fiber. The exit hook of `current`
//! destroys the listed values, newest first, when the thread exits.

use core::{
    cell::{Cell, UnsafeCell},
    fmt,
    mem::{self, MaybeUninit},
    ptr,
};

#[cfg(not(target_vendor = "apple"))]
use super::current;
use crate::{
    alloc::{GlobalAlloc, Layout, System},
    sys::os,
    unwind::abort_on_unwind,
};

/// The newest listed node of the calling thread, which heads the list of
/// all of them, or null.
#[thread_local]
static HEAD: Cell<*const Node> = Cell::new(ptr::null());

/// The part of a storage that lists it for destruction.
#[repr(C)]
struct Node {
    /// The next older listed node, or null.
    next: Cell<*const Self>,
    /// Destroys the value of the storage that holds this node; set when the
    /// node is listed, so that a new storage is all zero bytes.
    destroy: Cell<Option<unsafe fn(*const Self)>>,
}

impl Node {
    const fn new() -> Self {
        Self {
            next: Cell::new(ptr::null()),
            destroy: Cell::new(None),
        }
    }
}

/// The state of a value in a storage.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    /// Not initialized yet; zero, as a new storage is all zero bytes.
    Initial,
    Alive,
    /// Destroyed, or being destroyed; accesses fail from then on.
    Destroyed,
}

/// Returns whether a native thread-local can hold a `T` in place. Windows
/// aligns each thread's block of thread-locals only as its heap does, to
/// two words; values that need more live in a [`Boxed`] block there, and
/// under Miri, so that it checks that path too. Not public API.
#[doc(hidden)]
#[must_use]
pub const fn inline<T>() -> bool {
    !(cfg!(windows) || cfg!(miri))
        || mem::align_of::<T>() <= 2 * mem::size_of::<usize>()
}

/// Returns the provided value if there is one, and runs the initializer
/// otherwise.
fn take_or_init<T>(provided: Option<&mut Option<T>>, init: fn() -> T) -> T {
    provided
        .and_then(Option::take)
        .unwrap_or_else(|| abort_on_unwind(init))
}

/// Lists the storage of `node`, for `destroy` when the thread exits, and
/// makes sure that the exit hook runs then.
///
/// # Safety
///
/// `node` must be the node of an unlisted storage in a `#[thread_local]`
/// static of the calling thread, with the provenance of the whole storage,
/// and `destroy` must accept it.
#[cold]
#[inline(never)]
unsafe fn register(node: *const Node, destroy: unsafe fn(*const Node)) {
    arm_exit();
    // SAFETY: the caller passes a valid, aligned node of a live storage of
    // this thread, which only this thread touches.
    unsafe {
        (*node).destroy.set(Some(destroy));
        (*node).next.set(HEAD.get());
    }
    HEAD.set(node);
}

/// Makes sure that the calling thread runs [`run_destructors`] when it
/// exits, through the exit hook of `current`, which this creates the
/// thread's handle for if needed, running no user code.
#[cfg(not(target_vendor = "apple"))]
fn arm_exit() {
    current::arm_exit_hook();
}

/// As on other targets, but through `_tlv_atexit`: dyld frees native
/// thread-locals from a pthread key destructor of a key made before any of
/// litestd's, so the exit hook would find them gone, and dyld runs the
/// functions of `_tlv_atexit` first.
#[cfg(target_vendor = "apple")]
fn arm_exit() {
    /// Whether the calling thread registered [`run_at_exit`].
    #[thread_local]
    static ARMED: Cell<bool> = Cell::new(false);

    /// Runs the thread's destructors as dyld's exit function.
    unsafe extern "C" fn run_at_exit(_: *mut u8) {
        // SAFETY: dyld calls this as the thread exits.
        unsafe { run_destructors() };
    }

    if !ARMED.replace(true) {
        crate::sys::tlv::at_exit(run_at_exit);
    }
}

/// Destroys the calling thread's listed values, newest first, including
/// values that the destructors list meanwhile.
///
/// # Safety
///
/// The thread must be exiting: nothing may borrow its values any more.
pub(super) unsafe fn run_destructors() {
    // Each round destroys the values listed so far; their destructors list
    // new values on the emptied head. A destroyed value is never listed
    // again, so each static adds at most one value per call and the loop
    // ends.
    loop {
        let mut node = HEAD.replace(ptr::null());
        if node.is_null() {
            return;
        }
        // One pass over the finite list taken above.
        while !node.is_null() {
            // SAFETY: listed nodes belong to live storages of this thread;
            // the node is read before its value is destroyed.
            let (next, destroy) =
                unsafe { ((*node).next.get(), (*node).destroy.get()) };
            if let Some(destroy) = destroy {
                // SAFETY: the list owns the value, which nothing borrows
                // while the thread exits, and `destroy` is the one listed
                // for it.
                unsafe { destroy(node) };
            }
            node = next;
        }
    }
}

/// The native thread-local of a value that is initialized lazily or must be
/// dropped. Not public API.
#[doc(hidden)]
#[repr(C)]
pub struct Storage<T> {
    /// First, so that the value's address is the static's.
    value: UnsafeCell<MaybeUninit<T>>,
    state: Cell<State>,
    node: Node,
}

impl<T> Storage<T> {
    /// Creates the storage of a value that is not initialized yet.
    #[doc(hidden)]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            value: UnsafeCell::new(MaybeUninit::uninit()),
            state: Cell::new(State::Initial),
            node: Node::new(),
        }
    }

    /// Returns the calling thread's value, initialized first from
    /// `provided` if that holds one and from `init` otherwise, or null while
    /// or after the value is destroyed.
    ///
    /// # Safety
    ///
    /// `self` must be a `#[thread_local]` static, as it may be listed for
    /// destruction when the thread exits.
    #[doc(hidden)]
    #[inline]
    pub unsafe fn get(
        &self,
        provided: Option<&mut Option<T>>,
        init: fn() -> T,
    ) -> *const T {
        // The slow path reports only whether the value is alive, so that
        // both paths address it through the static.
        if self.state.get() != State::Alive {
            // SAFETY: the caller's guarantee.
            let alive = unsafe { self.initialize(provided, init) };
            if !alive {
                return ptr::null();
            }
        }
        self.value.get().cast()
    }

    /// The slow path of `get`: returns whether the value is alive.
    ///
    /// # Safety
    ///
    /// As for `get`.
    #[cold]
    unsafe fn initialize(
        &self,
        provided: Option<&mut Option<T>>,
        init: fn() -> T,
    ) -> bool {
        if self.state.get() == State::Destroyed {
            return false;
        }
        let value = take_or_init(provided, init);
        let slot = self.value.get();
        match self.state.get() {
            State::Initial => {
                // SAFETY: nothing borrows the uninitialized value.
                unsafe { slot.write(MaybeUninit::new(value)) };
                self.state.set(State::Alive);
                if mem::needs_drop::<T>() {
                    let node = ptr::from_ref(self)
                        .wrapping_byte_add(mem::offset_of!(Self, node));
                    // SAFETY: the caller guarantees that `self` is a
                    // `#[thread_local]` static; its node, derived from all
                    // of it, is listed only here, on the first
                    // initialization.
                    unsafe { register(node.cast(), destroy::<T>) };
                }
            }
            State::Alive => {
                // `init` initialized this static too. As in std, the outer
                // value stays, and the inner one, whose borrows ended within
                // `init`, is dropped.
                // SAFETY: `slot` points into `self`, so it is valid and
                // aligned; the value is initialized and borrowed by nothing.
                let mut inner =
                    unsafe { slot.replace(MaybeUninit::new(value)) };
                // SAFETY: as above.
                unsafe { inner.assume_init_drop() };
            }
            // Only the thread's exit destroys values, never while an
            // initializer runs; this arm just keeps the value unreachable.
            State::Destroyed => {
                drop(value);
                return false;
            }
        }
        true
    }
}

/// Destroys the value of the `Storage<T>` that holds `node`.
///
/// # Safety
///
/// `node` must be the node of a listed `Storage<T>` of the calling thread,
/// with the provenance of the whole storage, whose value is alive and
/// borrowed by nothing.
unsafe fn destroy<T>(node: *const Node) {
    let offset = mem::offset_of!(Storage<T>, node);
    // SAFETY: the caller passes the node of a storage with a live value,
    // and the offset leads back to its start, a valid and aligned
    // `Storage<T>`. The state changes first, so that the value's destructor
    // cannot reach the value.
    unsafe {
        let storage = node.byte_sub(offset).cast::<Storage<T>>();
        (*storage).state.set(State::Destroyed);
        ptr::drop_in_place((*storage).value.get().cast::<T>());
    }
}

/// The native thread-local of a value that lives in a heap block from
/// [`System`], because its alignment exceeds what [`inline`] allows. Not
/// public API.
#[doc(hidden)]
#[repr(C)]
pub struct Boxed<T> {
    /// The value's block while the value is alive, and null otherwise.
    value: Cell<*mut T>,
    state: Cell<State>,
    node: Node,
}

impl<T> Boxed<T> {
    /// Creates the storage of a value that is not initialized yet.
    #[doc(hidden)]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            value: Cell::new(ptr::null_mut()),
            state: Cell::new(State::Initial),
            node: Node::new(),
        }
    }

    /// As [`Storage::get`].
    ///
    /// # Safety
    ///
    /// As for [`Storage::get`].
    #[doc(hidden)]
    #[inline]
    pub unsafe fn get(
        &self,
        provided: Option<&mut Option<T>>,
        init: fn() -> T,
    ) -> *const T {
        let block = self.value.get();
        if !block.is_null() {
            return block;
        }
        // SAFETY: the caller's guarantee.
        unsafe { self.initialize(provided, init) }
    }

    /// The slow path of `get`.
    ///
    /// # Safety
    ///
    /// As for `get`.
    #[cold]
    unsafe fn initialize(
        &self,
        provided: Option<&mut Option<T>>,
        init: fn() -> T,
    ) -> *const T {
        if self.state.get() == State::Destroyed {
            return ptr::null();
        }
        let block = new_block(take_or_init(provided, init));
        match self.state.get() {
            State::Initial => {
                self.value.set(block);
                self.state.set(State::Alive);
                let node = ptr::from_ref(self)
                    .wrapping_byte_add(mem::offset_of!(Self, node));
                // SAFETY: as in `Storage::initialize`; the block must be
                // freed even if the value needs no drop.
                unsafe { register(node.cast(), destroy_boxed::<T>) };
            }
            State::Alive => {
                // As in `Storage::initialize`, the inner value goes.
                // SAFETY: the inner block holds a live value that nothing
                // borrows any more.
                unsafe { free_block(self.value.replace(block)) };
            }
            State::Destroyed => {
                // SAFETY: the new block is ours alone.
                unsafe { free_block(block) };
                return ptr::null();
            }
        }
        block
    }
}

/// Moves `value` into a new block from [`System`], or to a dangling but
/// aligned address if it is zero-sized. Aborts if memory runs out, as std
/// does for thread-locals.
// `System` aligns blocks as the layout asks, here for `T`.
#[allow(clippy::cast_ptr_alignment)]
fn new_block<T>(value: T) -> *mut T {
    let layout = Layout::new::<T>();
    let block = if layout.size() == 0 {
        ptr::dangling_mut()
    } else {
        // SAFETY: the layout is not zero-sized.
        let block = unsafe { System.alloc(layout) }.cast::<T>();
        if block.is_null() {
            os::abort();
        }
        block
    };
    // SAFETY: the block is valid for a `T` and aligned for it.
    unsafe { block.write(value) };
    block
}

/// Drops the value in `block` and frees the block.
///
/// # Safety
///
/// `block` must come from [`new_block`] and hold a live value that nothing
/// uses any more.
unsafe fn free_block<T>(block: *mut T) {
    let layout = Layout::new::<T>();
    // SAFETY: the caller hands over the value, whose block `new_block`
    // allocated with this layout unless it is zero-sized.
    unsafe {
        ptr::drop_in_place(block);
        if layout.size() != 0 {
            System.dealloc(block.cast(), layout);
        }
    }
}

/// Destroys the value of the `Boxed<T>` that holds `node`.
///
/// # Safety
///
/// As for [`destroy`], with a `Boxed<T>`.
unsafe fn destroy_boxed<T>(node: *const Node) {
    let offset = mem::offset_of!(Boxed<T>, node);
    // SAFETY: as in `destroy`, for a `Boxed<T>`, whose live value's block
    // becomes unreachable before the value's destructor runs.
    unsafe {
        let storage = node.byte_sub(offset).cast::<Boxed<T>>();
        (*storage).state.set(State::Destroyed);
        let block = (*storage).value.replace(ptr::null_mut());
        debug_assert!(!block.is_null());
        free_block(block);
    }
}

impl<T> fmt::Debug for Storage<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Storage").finish_non_exhaustive()
    }
}

impl<T> fmt::Debug for Boxed<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Boxed").finish_non_exhaustive()
    }
}
