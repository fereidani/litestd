//! Thread-local keys on fiber-local storage, passed around as `usize`. A key
//! holds one pointer per fiber, null until set; a thread that never becomes
//! a fiber has exactly one.

use core::ffi::c_void;

use windows_sys::Win32::System::Threading::{
    FLS_OUT_OF_INDEXES, FlsAlloc, FlsFree, FlsGetValue, FlsSetValue,
    IsThreadAFiber,
};

/// Runs when a thread exits while a hook key holds a non-null value.
pub(crate) trait Hook {
    /// Called once on the exiting thread with the value its slot held; the
    /// slot reads as that value or null during the call, and null after.
    /// Skipped on fibers. A non-fiber thread that deletes a fiber gets a call
    /// with that fiber's value while its own slots are current.
    ///
    /// # Safety
    ///
    /// `value` is a value the thread stored in the key.
    unsafe fn run(value: *mut u8);
}

/// Creates a key whose non-null values are passed to `H::run` when their
/// thread exits, or returns `None` if the process ran out of keys.
pub(crate) fn create_with_hook<H: Hook>() -> Option<usize> {
    // SAFETY: `run_hook` has the signature the system expects of a callback.
    let index = unsafe { FlsAlloc(Some(run_hook::<H>)) };
    if index == FLS_OUT_OF_INDEXES {
        None
    } else {
        usize::try_from(index).ok()
    }
}

unsafe extern "system" fn run_hook<H: Hook>(value: *const c_void) {
    // Deleting a fiber runs its callbacks on another fiber, whose slots the
    // hook would use instead; as std does, skip them on fibers and leak.
    // SAFETY: `IsThreadAFiber` has no preconditions.
    if unsafe { IsThreadAFiber() } != 0 {
        return;
    }
    // SAFETY: the system passes the value the exiting thread stored; an
    // unwinding panic aborts at this `extern "system"` boundary.
    unsafe { H::run(value.cast_mut().cast()) }
}

/// Converts a key from `create_with_hook` back to the system's type.
#[allow(clippy::cast_possible_truncation)]
const fn raw(key: usize) -> u32 {
    // `key` came from a `u32`, so the conversion is exact.
    key as u32
}

/// Frees a key.
///
/// # Safety
///
/// `key` must come from this module, hold no non-null value on any thread
/// (`FlsFree` would pass it to the hook), and never be used again.
pub(crate) unsafe fn destroy(key: usize) {
    // SAFETY: the caller guarantees that `key` is live.
    let r = unsafe { FlsFree(raw(key)) };
    // Only an invalid index makes this fail.
    debug_assert_ne!(r, 0);
}

/// Returns the calling fiber's value in `key`.
///
/// # Safety
///
/// `key` must come from this module and not be destroyed.
#[inline]
pub(crate) unsafe fn get(key: usize) -> *mut u8 {
    // SAFETY: the caller guarantees that `key` is live.
    unsafe { FlsGetValue(raw(key)) }.cast()
}

/// Stores the calling fiber's value in `key`, returning `false` if the
/// system could not allocate room for it; a slot stored to before has room.
///
/// # Safety
///
/// `key` must come from this module and not be destroyed. For a hook key,
/// `value` must be null or valid for the hook when the thread exits.
#[inline]
pub(crate) unsafe fn set(key: usize, value: *mut u8) -> bool {
    // SAFETY: the caller guarantees that `key` is live; `value` is only stored.
    unsafe { FlsSetValue(raw(key), value.cast::<c_void>()) != 0 }
}
