//! WebAssembly. On WASI, wasi-libc provides the POSIX calls of `unix`, and
//! `unsupported` stands in for sockets and child processes;
//! wasm32-unknown-unknown has no OS, and `unknown` and `unsupported` behave
//! as std does there. Without the `atomics` target feature one thread runs,
//! so the futex and the thread-local keys here need no synchronization.
//! With it, threads share the memory: the futex waits with
//! `memory.atomic.wait32`, wasi-libc's pthreads start threads on WASI, and
//! on wasm32-unknown-unknown, where the host starts them as std expects,
//! the keys live in `#[thread_local]` statics.

// These stand in for OS calls with the other backends' signatures, which
// are not `const`; were they `const`, clippy would ask the portable callers
// to be too, unlike std's.
#![allow(clippy::missing_const_for_fn, clippy::unnecessary_wraps)]

// WASI's `env` guards the environment with a lock.
#[cfg(any(
    all(target_os = "wasi", feature = "env"),
    feature = "stdio",
    feature = "sync",
    feature = "thread"
))]
pub(crate) mod futex;
// On WASI with threads, `unix` provides both, on pthreads.
#[cfg(all(
    feature = "thread",
    not(all(target_os = "wasi", target_feature = "atomics"))
))]
pub(crate) mod thread;
#[cfg(all(
    feature = "thread",
    not(all(target_os = "wasi", target_feature = "atomics"))
))]
pub(crate) mod tls;

#[cfg(target_os = "unknown")]
mod unknown;

#[cfg(target_os = "unknown")]
pub(crate) use self::unknown::*;
#[cfg(target_os = "wasi")]
pub(crate) use super::unix::*;
#[cfg(all(target_os = "unknown", feature = "env"))]
pub(crate) use super::unsupported::env;
#[cfg(all(target_os = "unknown", feature = "fs"))]
pub(crate) use super::unsupported::fs;
#[cfg(feature = "net")]
pub(crate) use super::unsupported::net;
#[cfg(all(target_os = "unknown", feature = "io"))]
pub(crate) use super::unsupported::pipe;
#[cfg(feature = "command")]
pub(crate) use super::unsupported::process;

/// Returns whether the calling thread is the main thread: the one that runs
/// on the stack the linker reserved, from `__stack_low` to `__stack_high`.
/// The threads that wasi-libc and the host start run on stacks from the
/// heap.
#[cfg(all(target_feature = "atomics", feature = "thread"))]
pub(crate) fn is_main() -> bool {
    unsafe extern "C" {
        static __stack_low: u8;
        static __stack_high: u8;
    }
    let marker = 0u8;
    let here = (&raw const marker).addr();
    (&raw const __stack_low).addr() <= here
        && here < (&raw const __stack_high).addr()
}
