//! Linux and Android: the futex, and the shared POSIX code of `sys::unix`.

// `stdio` locks the standard streams with it, and `env` the environment.
#[cfg(any(
    feature = "env",
    feature = "stdio",
    feature = "sync",
    feature = "thread"
))]
pub(crate) mod futex;

pub(crate) use super::unix::*;
