//! Thread identifiers.

use core::{num::NonZero, sync::atomic::Ordering::Relaxed};

use crate::sys::os;

/// A unique identifier for a running thread.
///
/// `ThreadId`s are never reused and unrelated to the OS's thread ids. Get
/// one from [`Thread::id`](super::Thread::id).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ThreadId(NonZero<u64>);

impl ThreadId {
    /// Returns an id that no thread had before.
    pub(crate) fn new() -> Self {
        let last = next();
        // Stop long before the counter could wrap and repeat an id: every
        // later thread stops too, and wrapping would take half the range.
        if last >= LIMIT {
            exhausted();
        }
        Self(NonZero::<u64>::MIN.saturating_add(last))
    }
}

/// Ids at or above this are never handed out.
#[cfg(target_has_atomic = "64")]
const LIMIT: u64 = u64::MAX / 2;
#[cfg(not(target_has_atomic = "64"))]
const LIMIT: u64 = (u32::MAX / 2) as u64;

/// Returns the number of ids handed out so far and counts one more.
#[cfg(target_has_atomic = "64")]
fn next() -> u64 {
    static COUNTER: core::sync::atomic::AtomicU64 =
        core::sync::atomic::AtomicU64::new(0);
    // Distinct ids need only the read-modify-write, in any memory order.
    COUNTER.fetch_add(1, Relaxed)
}

/// Returns the number of ids handed out so far and counts one more.
#[cfg(not(target_has_atomic = "64"))]
fn next() -> u64 {
    static COUNTER: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    // As above.
    u64::from(COUNTER.fetch_add(1, Relaxed))
}

/// Ends the process: every thread id was handed out. std panics here; with
/// 64-bit atomics it takes centuries of thread creation.
#[cold]
fn exhausted() -> ! {
    os::abort()
}
