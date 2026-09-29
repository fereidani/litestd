//! FreeBSD, NetBSD, OpenBSD and DragonFly: the futex of each, and the shared
//! POSIX code of `sys::unix`, which carries their variants.

// `stdio` locks the standard streams with it, and `env` the environment.
#[cfg(any(
    feature = "env",
    feature = "stdio",
    feature = "sync",
    feature = "thread"
))]
#[cfg_attr(target_os = "freebsd", path = "futex/freebsd.rs")]
#[cfg_attr(
    any(target_os = "netbsd", target_os = "openbsd"),
    path = "futex/relative.rs"
)]
#[cfg_attr(target_os = "dragonfly", path = "futex/dragonfly.rs")]
pub(crate) mod futex;

pub(crate) use super::unix::*;
