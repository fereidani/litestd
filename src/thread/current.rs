//! The calling thread's handle, and the teardown at thread exit.

use core::{
    ptr::{self, NonNull},
    time::Duration,
};

#[cfg(feature = "nightly")]
use super::local_native;
#[cfg(not(feature = "nightly"))]
use super::local_table::{self, Locals};
use super::{
    handle::{Name, Thread},
    id::ThreadId,
    key::LazyKey,
};
use crate::sys::{
    os, thread as native,
    tls::{self, Hook},
};

/// The calling thread's handle, as a pointer from [`Thread::into_slot`]
/// that owns a reference. The handle holds the thread-local values, which
/// with `nightly` are native thread-locals on a list of their own; at
/// thread exit the key's hook destroys them, newest first, and releases the
/// handle last, so that destructors can still use `current` and `park`.
static CURRENT: LazyKey = LazyKey::new(tls::create_with_hook::<Exit>);

/// Returns the slot's handle pointer, if the calling thread has one.
#[inline]
pub(super) fn get() -> Option<NonNull<u8>> {
    let key = CURRENT.get()?;
    // SAFETY: `key` came from `tls::create_with_hook` and is never
    // destroyed.
    NonNull::new(unsafe { tls::get(key) })
}

/// Returns the slot's handle pointer, creating the handle first if the
/// calling thread has none.
#[inline]
fn get_or_init() -> NonNull<u8> {
    get().unwrap_or_else(init)
}

/// Gets a handle to the thread that invokes it.
///
/// It works on every thread, including the main thread, named `"main"`, and
/// threads created by other code. The handle is created on first use, and
/// later calls make new handles to it without allocating or an atomic
/// operation.
#[must_use]
#[inline]
pub fn current() -> Thread {
    // SAFETY: `get_or_init` returns the calling thread's slot pointer.
    unsafe { Thread::clone_slot(get_or_init()) }
}

/// Returns the calling thread's thread-local state, creating its handle
/// first if needed. The pointer stays valid while the thread runs code, and
/// until the exit hook releases the handle.
#[cfg(not(feature = "nightly"))]
pub(super) fn locals() -> *mut Locals {
    // SAFETY: the slot's handle is the calling thread's.
    unsafe { Thread::locals(get_or_init()) }
}

/// Makes sure that the exit hook runs when the calling thread exits, by
/// creating the thread's handle if it has none.
#[cfg(all(feature = "nightly", not(target_vendor = "apple")))]
pub(super) fn arm_exit_hook() {
    get_or_init();
}

/// Creates the calling thread's handle and caches it in the slot, returning
/// the slot's pointer. It runs no user code and no global allocator, so it
/// cannot recurse into `current`.
#[cold]
fn init() -> NonNull<u8> {
    let name = if native::is_main() {
        Name::Main
    } else {
        Name::Unnamed
    };
    store(Thread::new(ThreadId::new(), name))
}

/// Caches `thread` as the calling thread's handle, before a spawned
/// thread's closure runs.
pub(super) fn set_current(thread: Thread) {
    store(thread);
}

/// Stores `thread` in the slot, returning the pointer that the slot owns.
///
/// Aborts, as std does, if the process is out of keys or memory for the
/// slot: a thread without a handle could neither keep thread-locals nor be
/// unparked.
fn store(thread: Thread) -> NonNull<u8> {
    let Some(key) = CURRENT.force() else {
        os::abort()
    };
    let raw = thread.into_slot();
    // SAFETY: the hook accepts a pointer from `into_slot` that owns its
    // reference.
    if !unsafe { tls::set(key, raw.as_ptr()) } {
        os::abort();
    }
    raw
}

/// The thread-exit hook of `CURRENT`.
struct Exit;

impl Hook for Exit {
    unsafe fn run(raw: *mut u8) {
        // The system passes only values that are not null.
        let (Some(key), Some(slot)) = (CURRENT.get(), NonNull::new(raw)) else {
            return;
        };
        // SAFETY: `key` came from `tls::create_with_hook` and is never
        // destroyed.
        let own = unsafe { tls::get(key) };
        if !own.is_null() && own != raw {
            // Windows passes a deleted fiber's values to the fiber that
            // deletes it, while that fiber's slots are the current ones: the
            // handle there is the caller's own. Its values must stay intact,
            // so the deleted fiber's leak.
            return;
        }
        // Native thread-locals belong to the OS thread, so only its exit may
        // destroy them. POSIX clears the slot before the hook and has no
        // fibers; Windows keeps the handle in the slot while the thread
        // exits, and finds null there when a thread without a handle of its
        // own deletes a fiber. On Apple targets `_tlv_atexit` destroyed them
        // before dyld freed their memory, which this hook must not touch.
        #[cfg(feature = "nightly")]
        let exiting =
            !cfg!(target_vendor = "apple") && (!cfg!(windows) || own == raw);
        // pthreads clear the slot before the hook; put the handle back so
        // that destructors find it. The slot was written before, so this
        // cannot fail.
        // SAFETY: `raw` is the slot's handle, which the hook accepts.
        let restored = unsafe { tls::set(key, raw) };
        debug_assert!(restored);
        #[cfg(not(feature = "nightly"))]
        // SAFETY: `raw` owns the handle's reference, and this is the thread
        // that the handle names.
        unsafe {
            local_table::run_destructors(Thread::locals(slot));
        }
        #[cfg(feature = "nightly")]
        if exiting {
            // SAFETY: the thread is exiting, so its own code is done.
            unsafe { local_native::run_destructors() };
        }
        // SAFETY: as above, and nothing is stored in the slot but handles.
        let cleared = unsafe { tls::set(key, ptr::null_mut()) };
        debug_assert!(cleared);
        // A later `current` creates a new handle and arms the hook again.
        // SAFETY: the slot held this pointer until just now, on this thread.
        unsafe { Thread::release_slot(slot) };
    }
}

/// Blocks unless or until the current thread's token is made available.
///
/// It may also return spuriously, so park in a loop. As in std, `unpark`
/// makes the token available with `Release`, and the `park` that consumes
/// it synchronizes with every earlier `unpark` of the thread.
pub fn park() {
    // Borrows the parker from the slot; no reference count changes.
    // SAFETY: the slot's reference is only released by the exit hook, which
    // cannot run during this call.
    unsafe { Thread::borrow_slot(get_or_init()) }
        .parker()
        .park();
}

/// Blocks unless or until the current thread's token is made available or
/// roughly `dur` elapsed, rounded up to whole milliseconds on Windows. It may
/// wake spuriously; see [`park`].
pub fn park_timeout(dur: Duration) {
    // SAFETY: as in `park`.
    unsafe { Thread::borrow_slot(get_or_init()) }
        .parker()
        .park_timeout(dur);
}
