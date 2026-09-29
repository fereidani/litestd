//! Owned permissions to join threads.

use core::fmt;

use super::{Result, Thread, spawn::JoinInner};
#[cfg(windows)]
use crate::os::windows::io::{AsRawHandle, IntoRawHandle, RawHandle};
#[cfg(any(unix, windows))]
use crate::sys::thread as native;

/// An owned permission to join on a thread (block on its termination).
///
/// Dropping it detaches the thread.
pub struct JoinHandle<T>(pub(super) JoinInner<'static, T>);

// SAFETY: the handle gives out a `T` only by value, from `join`, and
// spawning requires `T: Send`; the rest is the native thread and a
// counted, internally synchronized packet. std has the same impls.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl<T> Send for JoinHandle<T> {}
// SAFETY: shared references only read the thread handle and an atomic
// count.
unsafe impl<T> Sync for JoinHandle<T> {}

impl<T> JoinHandle<T> {
    /// Extracts a handle to the underlying thread.
    #[must_use]
    // std's `thread` is not `const`.
    #[allow(clippy::missing_const_for_fn)]
    pub fn thread(&self) -> &Thread {
        self.0.thread()
    }

    /// Waits for the associated thread to finish, including its
    /// thread-local destructors. Everything the thread did happens before
    /// `join` returns.
    ///
    /// # Errors
    ///
    /// Never fails: a panicking thread aborts the process. Joining the
    /// calling thread's own handle aborts too, where std may panic.
    #[inline]
    pub fn join(self) -> Result<T> {
        // Visibly `Ok`, so that callers' `unwrap` compiles to nothing.
        Ok(self.0.join())
    }

    /// Checks if the associated thread has finished running its main
    /// function, without blocking; [`join`](Self::join) then returns soon.
    // std's `is_finished` is not `#[must_use]`.
    #[allow(clippy::must_use_candidate)]
    pub fn is_finished(&self) -> bool {
        self.0.is_finished()
    }
}

/// Access to the native thread for `os::unix::thread::JoinHandleExt`, and
/// on Windows for the `AsRawHandle` and `IntoRawHandle` impls below.
#[cfg(any(unix, windows))]
impl<T> JoinHandle<T> {
    /// The native thread, which stays owned by the handle.
    pub(crate) const fn as_native(&self) -> &native::Thread {
        self.0.native()
    }

    /// Gives up the handle's share of the closure's result and returns the
    /// native thread, neither joined nor detached.
    pub(crate) fn into_native(self) -> native::Thread {
        self.0.into_native()
    }
}

#[cfg(windows)]
impl<T> AsRawHandle for JoinHandle<T> {
    #[inline]
    fn as_raw_handle(&self) -> RawHandle {
        self.as_native().as_raw()
    }
}

#[cfg(windows)]
impl<T> IntoRawHandle for JoinHandle<T> {
    /// Consumes the handle and returns the thread's handle, which the
    /// caller must close. The thread keeps running.
    #[inline]
    fn into_raw_handle(self) -> RawHandle {
        self.into_native().into_raw()
    }
}

impl<T> fmt::Debug for JoinHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JoinHandle").finish_non_exhaustive()
    }
}
