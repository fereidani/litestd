//! Spawning and joining threads.
//!
//! A spawn allocates the new thread's [`Thread`] handle, which can outlive
//! everything else, and a [`Spawn`] that carries the closure to the thread
//! and its result back, shared with the join handle through a reference
//! count; code that does not know the closure type uses its [`Packet`]
//! prefix. The thread consumes the closure, stores the result and releases
//! its reference. The last owner drops what is left of the result, frees
//! the packet and tells a scope that the thread is done; by then nothing
//! borrows from the scope, as the thread's frames hold only raw pointers.

use core::{
    alloc::Layout,
    cell::UnsafeCell,
    marker::PhantomData,
    mem::ManuallyDrop,
    ptr::{self, NonNull},
    sync::atomic::{
        AtomicUsize,
        Ordering::{Acquire, Release},
    },
};

use alloc_crate::{boxed::Box, sync::Arc};

use super::{
    Builder, current,
    handle::{Name, Thread},
    id::ThreadId,
    scoped::ScopeData,
};
use crate::{
    alloc::dealloc,
    io,
    sys::{
        os,
        thread::{self as native, Start},
    },
};

/// The stack size without [`Builder::stack_size`], as in std.
const DEFAULT_STACK_SIZE: usize = 2 * 1024 * 1024;

/// The part of a spawn that depends on neither the closure nor the result.
#[repr(C)]
struct Header {
    /// What the native thread calls; must stay the first field.
    start: Start,
    /// The owners: the thread until it stored its result, and the join
    /// handle.
    refs: AtomicUsize,
    /// The layout of the whole [`Spawn`], for code that does not know it.
    layout: Layout,
    /// The new thread's handle.
    thread: Thread,
    /// The scope of a scoped thread.
    scope: Option<Arc<ScopeData>>,
}

/// What the join handle sees of a spawn.
#[repr(C)]
struct Packet<T> {
    header: Header,
    result: UnsafeCell<Option<T>>,
}

/// A spawn with its closure, allocated once.
#[repr(C)]
struct Spawn<F, T> {
    packet: Packet<T>,
    /// Read exactly once, by the new thread, or dropped if the thread could
    /// not be created.
    f: ManuallyDrop<F>,
}

/// Spawns a thread that runs `f`, in `scope` if one is given.
///
/// # Safety
///
/// The thread must not outlive anything that `F` or `T` borrows: either the
/// caller joins it in time, or `scope` waits for it.
pub(super) unsafe fn spawn<'scope, F, T>(
    builder: Builder,
    scope: Option<&Arc<ScopeData>>,
    f: F,
) -> io::Result<JoinInner<'scope, T>>
where
    F: FnOnce() -> T + Send,
    T: Send,
{
    let Builder { name, stack_size } = builder;
    let header = Header::new(
        name.map_or(Name::Unnamed, Name::Given),
        scope,
        Layout::new::<Spawn<F, T>>(),
        run::<F, T>,
    );
    let packet = Packet::<T> {
        header,
        result: UnsafeCell::new(None),
    };
    let f = ManuallyDrop::new(f);
    let spawn = NonNull::from(Box::leak(Box::new(Spawn { packet, f })));
    // SAFETY: the header is the first field of the live `Spawn`, which
    // holds one reference for the thread and one for the handle, and `run`
    // matches its type. The caller vouches for the lifetimes.
    match unsafe { launch(spawn.cast(), stack_size) } {
        Ok(native) => Ok(JoinInner {
            native,
            packet: spawn.cast(),
            scope: PhantomData,
        }),
        Err(error) => {
            // SAFETY: the thread never started, so the box is ours alone
            // and still holds the closure.
            let mut spawn = unsafe { Box::from_raw(spawn.as_ptr()) };
            // SAFETY: the closure is dropped here and never read.
            unsafe { ManuallyDrop::drop(&mut spawn.f) };
            Err(error)
        }
    }
}

impl Header {
    fn new(
        name: Name,
        scope: Option<&Arc<ScopeData>>,
        layout: Layout,
        main: unsafe fn(NonNull<Start>),
    ) -> Self {
        Self {
            start: Start { main },
            refs: AtomicUsize::new(2),
            layout,
            thread: Thread::new(ThreadId::new(), name),
            scope: scope.cloned(),
        }
    }
}

/// Counts the thread in its scope and starts it.
///
/// # Safety
///
/// `header` must head a live [`Spawn`] whose `start.main` accepts it, with
/// one reference reserved for the new thread.
unsafe fn launch(
    header: NonNull<Header>,
    stack_size: Option<usize>,
) -> io::Result<native::Thread> {
    // SAFETY: the handle's reference keeps the header alive during this
    // call, and the new thread only reads it or touches the atomic count.
    let scope = unsafe { header.as_ref() }.scope.as_deref();
    // Count the thread before it can finish and uncount itself.
    if let Some(scope) = scope {
        scope.increment()?;
    }
    let stack_size = stack_size.unwrap_or(DEFAULT_STACK_SIZE);
    // SAFETY: the caller vouches for the header, whose first field is
    // `Start`.
    let native = unsafe { native::Thread::new(stack_size, header.cast()) };
    if native.is_err() {
        if let Some(scope) = scope {
            scope.decrement();
        }
    }
    native
}

/// The new thread's entry point.
///
/// # Safety
///
/// `start` must head a [`Spawn<F, T>`] that holds a reference for this
/// thread.
unsafe fn run<F, T>(start: NonNull<Start>)
where
    F: FnOnce() -> T,
{
    let spawn = start.cast::<Spawn<F, T>>();
    // SAFETY: this thread's reference keeps the spawn alive until
    // `release`, and `Start` heads the `Header`.
    unsafe { enter(start.cast()) };
    // SAFETY: the closure is read exactly once, here; the spawner does not
    // touch it after a successful spawn.
    let f = unsafe {
        ManuallyDrop::into_inner(ptr::read(&raw const (*spawn.as_ptr()).f))
    };
    let result = f();
    // SAFETY: only this thread touches the result until it releases its
    // reference, and the join handle reads it only after that.
    unsafe { (*spawn.as_ptr()).packet.result.get().write(Some(result)) };
    // The closure was consumed and the result moved into the packet, so no
    // borrow of the spawner's data remains in this frame.
    // SAFETY: this thread owns a reference and does not use the spawn
    // afterwards.
    unsafe { release::<T>(spawn.cast()) };
}

/// Prepares a new thread: caches its handle and names it.
///
/// # Safety
///
/// `header` must be live for the duration of the call.
unsafe fn enter(header: NonNull<Header>) {
    // SAFETY: the caller keeps the header alive; its `thread` field never
    // changes.
    let thread = unsafe { header.as_ref() }.thread.clone();
    if let Some(name) = thread.given_name() {
        native::set_name(name);
    }
    current::set_current(thread);
}

/// Releases one reference to a packet, freeing it if it was the last.
///
/// # Safety
///
/// The caller must own a reference to the live `packet` and must not use
/// the packet afterwards.
unsafe fn release<T>(packet: NonNull<Packet<T>>) {
    // SAFETY: the caller owns a reference.
    if !unsafe { release_ref(packet.cast()) } {
        return;
    }
    // Drop the result first: it may borrow from the scope, which learns in
    // `free` that the thread is done.
    // SAFETY: the last owner has the packet to itself.
    unsafe { ptr::drop_in_place((*packet.as_ptr()).result.get()) };
    // SAFETY: as above; nothing reads the result any more.
    unsafe { free(packet.cast()) };
}

/// Drops one reference, returning `true` if it was the last.
///
/// # Safety
///
/// The caller must own a reference to the live header.
unsafe fn release_ref(header: NonNull<Header>) -> bool {
    // SAFETY: the caller's reference keeps the header alive.
    let refs = unsafe { &header.as_ref().refs };
    // `Release` orders this owner's accesses before the free. The last
    // owner's `Acquire` load reads its own decrement, which continues the
    // release sequence of every earlier one, making their accesses visible.
    // A load rather than a fence, which ThreadSanitizer cannot follow.
    if refs.fetch_sub(1, Release) != 1 {
        return false;
    }
    refs.load(Acquire);
    true
}

/// Frees a packet whose result was dropped, then tells its scope that the
/// thread is done.
///
/// # Safety
///
/// The caller must be the packet's last owner.
// Kept out of line: it does not depend on the closure type, and inlining it
// would copy it into every spawn's code.
#[inline(never)]
unsafe fn free(header: NonNull<Header>) {
    // SAFETY: the last owner may move the fields out.
    let Header {
        layout,
        thread,
        scope,
        ..
    } = unsafe { header.read() };
    // SAFETY: `spawn` allocated the block with the global allocator and
    // this layout, and no field is used after this.
    unsafe { dealloc(header.as_ptr().cast(), layout) };
    drop(thread);
    if let Some(scope) = scope {
        scope.decrement();
    }
}

/// The shared part of [`JoinHandle`](super::JoinHandle) and
/// [`ScopedJoinHandle`](super::ScopedJoinHandle).
pub(super) struct JoinInner<'scope, T> {
    native: native::Thread,
    /// The handle's reference to the packet.
    packet: NonNull<Packet<T>>,
    /// The borrows of a scoped thread's closure and result.
    scope: PhantomData<&'scope ()>,
}

impl<T> JoinInner<'_, T> {
    const fn header(&self) -> &Header {
        // SAFETY: the handle's reference keeps the header alive.
        unsafe { self.packet.cast::<Header>().as_ref() }
    }

    pub(super) const fn thread(&self) -> &Thread {
        &self.header().thread
    }

    /// The native thread, which stays owned by the handle.
    #[cfg(any(unix, windows))]
    pub(super) const fn native(&self) -> &native::Thread {
        &self.native
    }

    pub(super) fn is_finished(&self) -> bool {
        // The thread releases its reference right after storing its result.
        self.header().refs.load(Acquire) == 1
    }

    /// Waits for the thread and returns its closure's result, which the
    /// public `join` methods wrap in `Ok`: a thread never ends by unwinding,
    /// since that aborts the process at its native entry point.
    pub(super) fn join(self) -> T {
        let (native, packet) = self.into_parts();
        join_native(native);
        // SAFETY: the join made the thread's writes visible, and the thread
        // no longer touches the packet; this frame owns the handle's
        // reference.
        let result = unsafe { (*(*packet.as_ptr()).result.get()).take() };
        // SAFETY: this frame owns the handle's reference, released exactly
        // once here.
        unsafe { release(packet) };
        // The thread stores its result before it releases its reference and
        // terminates, so a joined thread left one, unless it ended without
        // returning (`pthread_exit`, say), where std panics and this aborts.
        let Some(result) = result else { os::abort() };
        result
    }

    /// Gives up the handle's share of the packet and returns the native
    /// thread, neither joined nor detached. The result is dropped here if
    /// the thread has finished, and otherwise by the thread.
    #[cfg(any(unix, windows))]
    pub(super) fn into_native(self) -> native::Thread {
        let (native, packet) = self.into_parts();
        // SAFETY: this frame owns the handle's reference, released exactly
        // once here.
        unsafe { release(packet) };
        native
    }

    /// Takes the handle apart without dropping it: returns the native thread
    /// and the handle's reference to the packet, which the caller releases.
    fn into_parts(self) -> (native::Thread, NonNull<Packet<T>>) {
        let this = ManuallyDrop::new(self);
        // SAFETY: `this` is never dropped, so the native thread is moved out
        // exactly once.
        let native = unsafe { ptr::read(&raw const this.native) };
        (native, this.packet)
    }
}

/// Waits for a native thread to terminate.
fn join_native(native: native::Thread) {
    if native.join().is_err() {
        // Joining fails only for the calling thread itself, where std
        // panics on some platforms.
        os::abort();
    }
}

impl<T> Drop for JoinInner<'_, T> {
    fn drop(&mut self) {
        // Dropping `native` afterwards detaches the thread.
        // SAFETY: the handle owns a reference, released exactly once here.
        unsafe { release(self.packet) };
    }
}
