//! Native threads.
//!
//! As in std, a thread detaches when its [`JoinHandle`] is dropped, the
//! process ends when the main thread returns, the main thread is named
//! `"main"`, and spawned threads get 2 MiB stacks unless
//! [`Builder::stack_size`] says otherwise. Unlike std, a panic that escapes
//! a thread aborts the process, [`JoinHandle::join`] always returns `Ok`,
//! [`panicking`] always returns `false`, and `RUST_MIN_STACK` is not read.
//!
//! ```
//! use litestd::thread;
//!
//! let handle = thread::spawn(|| 6 * 7);
//! assert_eq!(handle.join().unwrap(), 42);
//! ```

mod builder;
mod current;
mod functions;
mod handle;
mod id;
mod join_handle;
mod key;
mod local;
#[cfg(feature = "nightly")]
mod local_native;
#[cfg(not(feature = "nightly"))]
mod local_table;
mod parker;
mod scoped;
mod spawn;

use core::{any::Any, result};

use alloc_crate::boxed::Box;
pub use builder::Builder;
pub use current::{current, park, park_timeout};
pub use functions::{
    available_parallelism, panicking, sleep, spawn, yield_now,
};
pub use handle::Thread;
pub use id::ThreadId;
pub use join_handle::JoinHandle;
pub use local::{AccessError, LocalKey};
pub use scoped::{Scope, ScopedJoinHandle, scope};

/// Implementation details of the `thread_local!` macro. Not public API.
#[doc(hidden)]
pub mod local_impl {
    #[cfg(feature = "nightly")]
    pub use super::local_native::{Boxed, Storage, inline};
    #[cfg(not(feature = "nightly"))]
    pub use super::local_table::Storage;
}

/// A specialized [`Result`](result::Result) type for threads. Its `Err`, a
/// panic payload, never occurs: a panic that escapes a thread aborts.
pub type Result<T> = result::Result<T, Box<dyn Any + Send + 'static>>;
