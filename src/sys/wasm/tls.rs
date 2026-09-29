//! Thread-local keys of a module whose threads never exit while it runs:
//! the one thread without the `atomics` target feature, and with it the
//! threads that the host starts, which it ends from outside. Each key's slot
//! is a static, thread-local with threads, and, as for the Unix main
//! thread, hooks never run.

use core::{
    ptr,
    sync::atomic::{AtomicPtr, AtomicUsize, Ordering::Relaxed},
};

/// Code that would run when a thread exits while a hook key holds a
/// non-null value.
pub(crate) trait Hook {
    /// # Safety
    ///
    /// `value` is a value the thread stored in the key.
    #[allow(dead_code, reason = "no thread exits while the module runs")]
    unsafe fn run(value: *mut u8);
}

/// How many keys the module may create: litestd needs one.
const KEYS: usize = 4;

/// The number of keys created so far.
static CREATED: AtomicUsize = AtomicUsize::new(0);

/// The slots of the keys, null until set; each thread has its own.
#[cfg_attr(target_feature = "atomics", thread_local)]
static SLOTS: [AtomicPtr<u8>; KEYS] =
    [const { AtomicPtr::new(ptr::null_mut()) }; KEYS];

/// Creates a key, or returns `None` once every key is taken.
pub(crate) fn create_with_hook<H: Hook>() -> Option<usize> {
    // Only the count is shared, and nothing is published with it.
    let mut key = CREATED.load(Relaxed);
    // Each failed exchange means that another thread took a key, which
    // happens at most `KEYS` times, so the loop ends.
    while key < KEYS {
        match CREATED.compare_exchange(key, key + 1, Relaxed, Relaxed) {
            Ok(_) => return Some(key),
            Err(now) => key = now,
        }
    }
    None
}

/// Deletes a key, which stays taken.
///
/// # Safety
///
/// `key` must come from this module, and must never be used again.
pub(crate) unsafe fn destroy(key: usize) {
    if let Some(slot) = SLOTS.get(key) {
        slot.store(ptr::null_mut(), Relaxed);
    }
}

/// Returns the value in `key`.
///
/// # Safety
///
/// `key` must come from this module and not be destroyed.
#[inline]
pub(crate) unsafe fn get(key: usize) -> *mut u8 {
    SLOTS
        .get(key)
        .map_or(ptr::null_mut(), |slot| slot.load(Relaxed))
}

/// Stores the value in `key`; returns `false` only for a key that this
/// module did not create.
///
/// # Safety
///
/// `key` must come from this module and not be destroyed.
#[inline]
pub(crate) unsafe fn set(key: usize, value: *mut u8) -> bool {
    SLOTS.get(key).is_some_and(|slot| {
        slot.store(value, Relaxed);
        true
    })
}
