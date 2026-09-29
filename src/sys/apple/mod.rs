//! macOS 14.4 and later: the futex, and the shared POSIX code of `sys::unix`,
//! which carries the macOS variants of what differs from Linux.

// `stdio` locks the standard streams with it, and `env` the environment.
#[cfg(any(
    feature = "env",
    feature = "stdio",
    feature = "sync",
    feature = "thread"
))]
pub(crate) mod futex;

// Native thread-locals, with `nightly`, run their destructors from it.
#[cfg(all(feature = "nightly", feature = "thread"))]
pub(crate) mod tlv;

pub(crate) use super::unix::*;
