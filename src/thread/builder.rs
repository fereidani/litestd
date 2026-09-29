//! Thread configuration.

use alloc_crate::string::String;

use super::{JoinHandle, spawn};
use crate::io;

/// Thread factory, which can be used in order to configure the properties
/// of a new thread.
#[must_use = "must eventually spawn the thread"]
#[derive(Debug)]
pub struct Builder {
    /// The name of the new thread.
    pub(super) name: Option<String>,
    /// The requested stack size in bytes; see [`Builder::stack_size`].
    pub(super) stack_size: Option<usize>,
}

// std's builder methods are not `const`.
#[allow(clippy::missing_const_for_fn)]
impl Builder {
    /// Generates the base configuration for spawning a thread, from which
    /// configuration methods can be chained.
    // std's `Builder` has no `Default` implementation.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            name: None,
            stack_size: None,
        }
    }

    /// Names the thread-to-be.
    ///
    /// [`Thread::name`](super::Thread::name) returns the name as given, and
    /// the OS gets a best-effort copy: Linux keeps 15 bytes, macOS 63 and
    /// Windows 255 UTF-16 units. Unlike std, a NUL does not panic; the copy
    /// ends there.
    pub fn name(mut self, name: String) -> Self {
        self.name = Some(name);
        self
    }

    /// Sets the size of the stack (in bytes) for the new thread.
    ///
    /// The OS rounds it up to its granularity, and on Unix to at least
    /// `PTHREAD_STACK_MIN`. The default is 2 MiB, as in std, and
    /// `RUST_MIN_STACK` is not read.
    pub fn stack_size(mut self, size: usize) -> Self {
        self.stack_size = Some(size);
        self
    }

    /// Spawns a new thread by taking ownership of the `Builder`, and
    /// returns an [`io::Result`] to its [`JoinHandle`].
    ///
    /// # Errors
    ///
    /// Returns the OS error if the thread cannot be created.
    pub fn spawn<F, T>(self, f: F) -> io::Result<JoinHandle<T>>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        // SAFETY: `F` and `T` are `'static`, so the thread cannot outlive
        // anything they borrow.
        unsafe { self.spawn_unchecked(f) }
    }

    /// Spawns a new thread like [`Builder::spawn`], without its `'static`
    /// bounds.
    ///
    /// # Errors
    ///
    /// As for [`Builder::spawn`].
    ///
    /// # Safety
    ///
    /// The thread must not outlive anything its closure or result borrows,
    /// for example by being joined before that data is dropped.
    pub unsafe fn spawn_unchecked<F, T>(self, f: F) -> io::Result<JoinHandle<T>>
    where
        F: FnOnce() -> T + Send,
        T: Send,
    {
        // SAFETY: the caller guarantees that the thread does not outlive
        // what `F` and `T` borrow.
        Ok(JoinHandle(unsafe { spawn::spawn(self, None, f) }?))
    }
}
