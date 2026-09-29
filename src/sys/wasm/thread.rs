//! Threads of a module that cannot start them, as std has them: without the
//! `atomics` target feature the one thread, and with it on
//! wasm32-unknown-unknown the threads that the host starts. Spawning fails.

#[cfg(not(target_feature = "atomics"))]
use core::time::Duration;
use core::{convert::Infallible, num::NonZero, ptr::NonNull};

use crate::io;
#[cfg(not(target_os = "wasi"))]
use crate::sys::unsupported::UNSUPPORTED;

/// The header of the block a new thread would receive.
#[repr(C)]
pub(crate) struct Start {
    #[allow(dead_code, reason = "no thread ever starts")]
    pub(crate) main: unsafe fn(NonNull<Self>),
}

/// A spawned thread, of which there are none.
pub(crate) struct Thread(Infallible);

impl Thread {
    /// Fails: the module cannot start threads. On WASI with wasi-libc's
    /// `ENOTSUP`, as std reports it.
    ///
    /// # Safety
    ///
    /// None; the signature matches the other backends.
    pub(crate) unsafe fn new(
        _stack_size: usize,
        _start: NonNull<Start>,
    ) -> io::Result<Self> {
        #[cfg(target_os = "wasi")]
        return Err(io::Error::from_raw_os_error(libc::ENOTSUP));
        #[cfg(not(target_os = "wasi"))]
        Err(UNSUPPORTED)
    }

    pub(crate) fn join(self) -> io::Result<()> {
        match self.0 {}
    }
}

/// Returns whether the calling thread is the main thread, the only one.
#[cfg(not(target_feature = "atomics"))]
pub(crate) fn is_main() -> bool {
    true
}

#[cfg(target_feature = "atomics")]
pub(crate) use super::is_main;

/// Names the calling thread, which has no name the OS could keep.
pub(crate) fn set_name(_name: &str) {}

/// Blocks the calling thread for at least `dur`.
#[cfg(target_os = "wasi")]
pub(crate) fn sleep(dur: Duration) {
    super::time::sleep_until(super::time::monotonic().saturating_add(dur));
}

#[cfg(target_feature = "atomics")]
pub(crate) use super::futex::sleep;

/// Panics as std does: there is nothing to sleep on.
#[cfg(not(any(target_os = "wasi", target_feature = "atomics")))]
#[cold]
#[track_caller]
#[allow(clippy::panic, reason = "std panics here")]
pub(crate) fn sleep(_dur: Duration) {
    panic!("can't sleep")
}

/// Offers the rest of the time slice to other threads; on WASI the runtime
/// may run other work.
pub(crate) fn yield_now() {
    // SAFETY: `sched_yield` has no preconditions.
    #[cfg(target_os = "wasi")]
    let r = unsafe { libc::sched_yield() };
    // It cannot fail.
    #[cfg(target_os = "wasi")]
    debug_assert_eq!(r, 0);
}

/// Returns 1 on WASI, where wasi-libc reports one processor, as std does.
#[cfg(target_os = "wasi")]
pub(crate) fn available_parallelism() -> io::Result<NonZero<usize>> {
    Ok(NonZero::<usize>::MIN)
}

/// Fails without an OS to ask, as in std.
#[cfg(not(target_os = "wasi"))]
pub(crate) fn available_parallelism() -> io::Result<NonZero<usize>> {
    Err(io::const_error!(
        io::ErrorKind::NotFound,
        "the number of hardware threads is not known for the target platform"
    ))
}
