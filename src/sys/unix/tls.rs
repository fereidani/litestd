//! Thread-local keys on top of `pthread_key_create`, passed around as
//! `usize`. Each slot holds a pointer per thread, null until set.

use core::ffi::c_void;

/// Code that runs when a thread exits while a hook key holds a non-null
/// value.
pub(crate) trait Hook {
    /// Called on the exiting thread with the value its slot held, after
    /// pthreads cleared the slot. Storing a non-null value again reruns it,
    /// for at most `PTHREAD_DESTRUCTOR_ITERATIONS` passes over all keys (Miri
    /// keeps going until every slot is null).
    ///
    /// # Safety
    ///
    /// `value` is a value the thread stored in the key.
    unsafe fn run(value: *mut u8);
}

/// Creates a key whose non-null values are passed to `H::run` when their
/// thread exits, or returns `None` if the process ran out of keys.
pub(crate) fn create_with_hook<H: Hook>() -> Option<usize> {
    let mut key: libc::pthread_key_t = 0;
    // SAFETY: `key` is valid for writes, and `run_hook` has the signature
    // pthreads expect of a destructor.
    let r =
        unsafe { libc::pthread_key_create(&raw mut key, Some(run_hook::<H>)) };
    if r == 0 {
        usize::try_from(key).ok()
    } else {
        None
    }
}

unsafe extern "C" fn run_hook<H: Hook>(value: *mut c_void) {
    // SAFETY: pthreads pass the value the exiting thread stored. A panic
    // that unwinds out of the hook aborts at this `extern "C"` boundary.
    unsafe { H::run(value.cast()) }
}

/// Converts a key from `create_with_hook` back to the C type, which is a
/// signed `int` on the BSDs.
#[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
const fn raw(key: usize) -> libc::pthread_key_t {
    // `key` came from a `pthread_key_t`, so the conversion is exact.
    key as libc::pthread_key_t
}

/// Deletes a key.
///
/// # Safety
///
/// `key` must come from this module, no thread may hold a non-null value in
/// it, and it must never be used again.
pub(crate) unsafe fn destroy(key: usize) {
    // SAFETY: the caller guarantees that `key` is live.
    let r = unsafe { libc::pthread_key_delete(raw(key)) };
    // Only an invalid key makes this fail.
    debug_assert_eq!(r, 0);
}

/// Returns the calling thread's value in `key`.
///
/// # Safety
///
/// `key` must come from this module and not be destroyed.
#[inline]
pub(crate) unsafe fn get(key: usize) -> *mut u8 {
    // SAFETY: the caller guarantees that `key` is live.
    unsafe { libc::pthread_getspecific(raw(key)) }.cast()
}

/// Stores the calling thread's value in `key`. Returns `false` if the C
/// library could not allocate room, never for a slot stored into before.
///
/// # Safety
///
/// `key` must come from this module and not be destroyed. For a hook key,
/// `value` must be null or valid for the hook when the thread exits.
#[inline]
pub(crate) unsafe fn set(key: usize, value: *mut u8) -> bool {
    // SAFETY: the caller guarantees that `key` is live.
    unsafe { libc::pthread_setspecific(raw(key), value.cast()) == 0 }
}
