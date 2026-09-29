//! POSIX code shared by the Unix backends, and by WASI through wasi-libc,
//! which has no sockets before WASI 0.2, processes or signals, and threads
//! only with the `atomics` target feature, as on `wasm32-wasip1-threads`.

pub(crate) mod alloc;
#[cfg(feature = "env")]
pub(crate) mod env;
#[cfg(feature = "fs")]
pub(crate) mod fs;
#[cfg(all(unix, feature = "net"))]
pub(crate) mod net;
pub(crate) mod os;
#[cfg(feature = "io")]
pub(crate) mod pipe;
#[cfg(all(unix, feature = "command"))]
pub(crate) mod process;
// Where litestd reads, writes or creates descriptors. Miri does not emulate
// `signal`.
#[cfg(all(unix, any(feature = "io", feature = "stdio"), not(miri)))]
mod rt;
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
#[cfg(all(any(unix, target_feature = "atomics"), feature = "thread"))]
pub(crate) mod thread;
// The futex's deadlines need it, and `net` on Unix times out connection
// attempts.
#[cfg(any(
    all(unix, feature = "net"),
    feature = "sync",
    feature = "thread",
    feature = "time"
))]
pub(crate) mod time;
#[cfg(all(any(unix, target_feature = "atomics"), feature = "thread"))]
pub(crate) mod tls;
