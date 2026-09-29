//! Synchronization primitives.
//!
//! Every primitive is a futex algorithm on `sys::futex` whose uncontended
//! paths are one atomic operation; nothing allocates, and nothing is ever
//! poisoned. The lock algorithms are non-generic, so their slow paths are
//! compiled once. `atomic` needs no feature, `Arc` and `Weak` need `alloc`,
//! and the rest needs `sync`.

#[cfg(feature = "sync")]
#[macro_use]
mod macros;

#[cfg(feature = "sync")]
mod barrier;
#[cfg(feature = "sync")]
mod condvar;
#[cfg(feature = "sync")]
mod lazy_lock;
#[cfg(feature = "sync")]
mod mutex;
#[cfg(feature = "sync")]
mod once;
#[cfg(feature = "sync")]
mod once_lock;
#[cfg(feature = "sync")]
mod poison;
/// The number of times a thread re-reads a busy lock before sleeping.
///
/// As in std. Measured from 0 to 1000, it was best or within noise with two
/// to four contending threads; not spinning made contended operations up to
/// 40% slower, and longer spins only paid off with eight threads.
#[cfg(any(
    feature = "stdio",
    feature = "sync",
    all(any(unix, target_os = "wasi"), feature = "env")
))]
const SPIN_LIMIT: u32 = 100;

// The standard streams of `stdio` lock with it too.
#[cfg(any(feature = "stdio", feature = "sync"))]
pub(crate) mod raw_mutex;
// On Unix and WASI, `env` guards the process environment with it.
#[cfg(any(
    feature = "sync",
    all(any(unix, target_os = "wasi"), feature = "env")
))]
pub(crate) mod raw_rwlock;
#[cfg(feature = "sync")]
mod rwlock;

pub use core::sync::atomic;

#[cfg(feature = "alloc")]
pub use alloc_crate::sync::{Arc, Weak};

#[cfg(feature = "sync")]
pub use self::{
    barrier::{Barrier, BarrierWaitResult},
    condvar::{Condvar, WaitTimeoutResult},
    lazy_lock::LazyLock,
    mutex::{Mutex, MutexGuard},
    once::{Once, OnceState},
    once_lock::OnceLock,
    poison::{LockResult, PoisonError, TryLockError, TryLockResult},
    rwlock::{RwLock, RwLockReadGuard, RwLockWriteGuard},
};
