//! Free functions.

use core::{num::NonZero, time::Duration};

use super::{Builder, JoinHandle};
use crate::{io, sys::thread as native};

/// Panics because the OS refused to create a thread.
#[cold]
#[inline(never)]
#[track_caller]
#[allow(clippy::panic)]
pub(super) fn spawn_failed() -> ! {
    // std's `thread::spawn` and `Scope::spawn` panic when the OS fails to
    // create a thread.
    panic!("failed to spawn thread")
}

/// Spawns a new thread, returning a [`JoinHandle`] for it.
///
/// The thread is detached if the handle is dropped. A panic that escapes
/// `f` aborts the process.
///
/// # Panics
///
/// Panics if the OS fails to create a thread; use [`Builder::spawn`] to
/// handle the error.
#[track_caller]
pub fn spawn<F, T>(f: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let Ok(handle) = Builder::new().spawn(f) else {
        spawn_failed()
    };
    handle
}

/// Cooperatively gives up a timeslice to the OS scheduler.
pub fn yield_now() {
    native::yield_now();
}

/// Determines whether the current thread is unwinding because of a panic:
/// always `false`, since no litestd thread ever unwinds.
#[inline]
#[must_use]
// std's `panicking` is not `const`.
#[allow(clippy::missing_const_for_fn)]
pub fn panicking() -> bool {
    false
}

/// Puts the current thread to sleep for at least the specified amount of
/// time; any `Duration` works.
///
/// Linux uses `clock_nanosleep` on the monotonic clock and macOS
/// `nanosleep`, both resumed after signal handlers; Windows uses a
/// high-resolution waitable timer where available, and `Sleep` otherwise.
pub fn sleep(dur: Duration) {
    native::sleep(dur);
}

/// Returns an estimate of the default amount of parallelism a program
/// should use. It is not cached, so avoid calling it from hot code.
///
/// Linux counts the CPUs in the thread's affinity mask, capped at the
/// process's cgroup CPU quota rounded down to whole CPUs, but at least one;
/// finding a cgroup v1 quota may scan the mount table. macOS counts the online
/// CPUs and Windows the logical processors of the process's group, which
/// undercounts beyond 64. All of this matches std.
///
/// # Errors
///
/// Returns an error if the number of CPUs cannot be determined.
pub fn available_parallelism() -> io::Result<NonZero<usize>> {
    native::available_parallelism()
}
