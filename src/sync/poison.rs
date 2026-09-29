//! Lock poisoning types. litestd locks are never poisoned, so litestd never
//! produces the `Err` variants; the types exist for std compatibility.

use core::{error::Error, fmt};

/// A type of error which can be returned whenever a lock is acquired.
///
/// litestd never returns it, but [`PoisonError::new`] can create one.
pub struct PoisonError<T> {
    data: T,
}

/// An enumeration of possible errors associated with a [`TryLockResult`]
/// which can occur while trying to acquire a lock.
pub enum TryLockError<T> {
    /// The lock could not be acquired because another thread failed while
    /// holding the lock. litestd never returns this variant.
    Poisoned(PoisonError<T>),
    /// The lock could not be acquired at this time because the operation
    /// would otherwise block.
    WouldBlock,
}

/// A type alias for the result of a lock method which can be poisoned;
/// always `Ok` in litestd.
pub type LockResult<T> = Result<T, PoisonError<T>>;

/// A type alias for the result of a nonblocking locking method.
pub type TryLockResult<Guard> = Result<Guard, TryLockError<Guard>>;

/// The message std uses for a poisoned lock.
const POISONED: &str = "poisoned lock: another task failed inside";

// Not `const`, as in std.
#[allow(clippy::missing_const_for_fn)]
impl<T> PoisonError<T> {
    /// Creates a `PoisonError`.
    pub fn new(data: T) -> Self {
        Self { data }
    }

    /// Consumes this error, returning the associated data.
    pub fn into_inner(self) -> T {
        self.data
    }

    /// Returns a reference to the associated data.
    pub fn get_ref(&self) -> &T {
        &self.data
    }

    /// Returns a mutable reference to the associated data.
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.data
    }
}

impl<T> fmt::Debug for PoisonError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PoisonError").finish_non_exhaustive()
    }
}

impl<T> fmt::Display for PoisonError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(POISONED)
    }
}

impl<T> Error for PoisonError<T> {}

impl<T> From<PoisonError<T>> for TryLockError<T> {
    fn from(err: PoisonError<T>) -> Self {
        Self::Poisoned(err)
    }
}

impl<T> fmt::Debug for TryLockError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // std formats the variant name as a quoted string.
        let name = match self {
            Self::Poisoned(..) => "Poisoned(..)",
            Self::WouldBlock => "WouldBlock",
        };
        fmt::Debug::fmt(name, f)
    }
}

impl<T> fmt::Display for TryLockError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(match self {
            Self::Poisoned(..) => POISONED,
            Self::WouldBlock => {
                "try_lock failed because the operation would block"
            }
        })
    }
}

impl<T> Error for TryLockError<T> {
    #[allow(deprecated)]
    fn cause(&self) -> Option<&dyn Error> {
        // std reports the poison error through the deprecated `cause`, not
        // `source`; mirror it so that both methods answer as in std.
        match self {
            Self::Poisoned(err) => Some(err),
            Self::WouldBlock => None,
        }
    }
}
