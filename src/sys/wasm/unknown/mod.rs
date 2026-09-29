//! wasm32-unknown-unknown, which has no OS: memory from `memory.grow`, and
//! std's stand-ins for everything else.

pub(crate) mod alloc;
pub(crate) mod os;
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
#[cfg(any(feature = "sync", feature = "thread", feature = "time"))]
pub(crate) mod time;
