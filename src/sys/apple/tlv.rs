//! `_tlv_atexit`: dyld runs the functions it registers when the calling
//! thread exits, and at `exit` for the main thread, before it frees the
//! thread's native thread-locals.

use core::ptr;

unsafe extern "C" {
    fn _tlv_atexit(dtor: unsafe extern "C" fn(*mut u8), arg: *mut u8);
}

/// Makes the calling thread run `f` when it exits, before its native
/// thread-locals are freed. `f` gets a null argument.
pub(crate) fn at_exit(f: unsafe extern "C" fn(*mut u8)) {
    // SAFETY: `f` lives as long as the process, and dyld passes it the
    // argument given here, which it ignores.
    unsafe { _tlv_atexit(f, ptr::null_mut()) };
}
