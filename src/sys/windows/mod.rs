//! Windows 10 and later.

pub(crate) mod alloc;
#[cfg(feature = "fs")]
mod dir;
#[cfg(feature = "env")]
pub(crate) mod env;
#[cfg(feature = "fs")]
mod file;
#[cfg(feature = "fs")]
pub(crate) mod fs;
// `stdio` locks the standard streams with it.
#[cfg(any(feature = "stdio", feature = "sync", feature = "thread"))]
pub(crate) mod futex;
#[cfg(feature = "net")]
pub(crate) mod net;
pub(crate) mod os;
#[cfg(any(feature = "command", feature = "fs"))]
mod path;
#[cfg(feature = "io")]
pub(crate) mod pipe;
#[cfg(feature = "command")]
pub(crate) mod process;
#[cfg(feature = "fs")]
mod remove_dir_all;
#[cfg(any(
    feature = "stdio",
    all(
        feature = "panic-location",
        not(any(
            test,
            feature = "test-with-std",
            feature = "custom-panic-handler"
        ))
    )
))]
pub(crate) mod stdio;
#[cfg(feature = "io")]
pub(crate) mod terminal;
#[cfg(feature = "thread")]
pub(crate) mod thread;
#[cfg(any(feature = "sync", feature = "thread", feature = "time"))]
pub(crate) mod time;
#[cfg(feature = "thread")]
pub(crate) mod tls;
