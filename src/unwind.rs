//! Panic containment. A panic can unwind into litestd only in a std binary
//! built with `panic = "unwind"`; [`abort_on_unwind`] turns it into an abort
//! where litestd calls user code, and compiles to nothing otherwise.

use core::mem;

use crate::sys::os;

/// Calls `f`, aborting the process if `f` unwinds.
#[inline]
pub(crate) fn abort_on_unwind<R, F: FnOnce() -> R>(f: F) -> R {
    let guard = AbortOnDrop;
    let result = f();
    mem::forget(guard);
    result
}

/// Aborts when dropped, which happens only while unwinding: once `f`
/// returns, [`abort_on_unwind`] forgets it.
struct AbortOnDrop;

impl Drop for AbortOnDrop {
    #[cold]
    fn drop(&mut self) {
        os::abort();
    }
}
