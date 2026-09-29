//! std's OS-dependent API for `no_std` crates.
//!
//! `litestd` reimplements the parts of std that need an operating system
//! directly on OS primitives, with std's paths and signatures, and
//! re-exports `core` and `alloc` at std's paths, so a crate drops std with
//! one switch:
//!
//! ```ignore
//! #![cfg_attr(not(feature = "std"), no_std)]
//!
//! #[cfg(not(feature = "std"))]
//! #[macro_use]
//! extern crate litestd as std;
//! ```
//!
//! The `no_std` prelude lacks `Box`, `String`, `ToString`, `Vec` and
//! `ToOwned`: import them (`use std::vec::Vec;`) or glob-import
//! `std::prelude::rust_2024::*`; both forms compile with std too.
//!
//! # Differences from std
//!
//! Every panic terminates the process: `thread::JoinHandle::join` always
//! returns `Ok`, `thread::panicking` returns `false`, and nothing is ever
//! poisoned, though the poison types exist so std code compiles unchanged.
//! A panic unwinding into litestd, possible only in a std binary built with
//! `panic = "unwind"`, aborts the process.
//!
//! Standard output is unbuffered, like standard error: each `print!` or
//! write reaches the OS before it returns, one write per call when it fits
//! in 1 KiB, so no output is lost when the process ends. As in std, the
//! macros panic when a write fails, and on Windows a console gets the text
//! as UTF-16, so it shows every character whatever its code page.
//!
//! # Platforms
//!
//! Linux with glibc or musl, Android, Windows 10 version 1607 and later with
//! the GNU or MSVC toolchain, macOS 14.4 and later, FreeBSD 12.2 and later,
//! NetBSD 10 and later, OpenBSD 6.2 and later, DragonFly, and WebAssembly:
//! WASI 0.1 and 0.2 and `wasm32-unknown-unknown`; other targets fail to
//! compile. On each BSD, litestd waits with its own futex: `_umtx_op`,
//! NetBSD's `__futex`, `futex` and `umtx_sleep`. On macOS, litestd waits with
//! the `os_sync_wait_on_address` functions that macOS 14.4 introduced, and a
//! program that links them does not start on earlier releases.
//!
//! A WebAssembly module runs one thread unless it is built with the
//! `atomics` target feature, as `wasm32-wasip1-threads` is. Its threads then
//! wait with `memory.atomic.wait32`, whose intrinsic is unstable, so litestd
//! needs its `nightly` feature there. On `wasm32-unknown-unknown`, which has
//! no OS, litestd does what std does: most operations fail with
//! `Unsupported`, output goes nowhere, and reading a clock panics.
//!
//! # Features
//!
//! Each module is a feature, all enabled by default, and `nightly` is one
//! more, off by default. A module whose feature is off keeps the items that
//! need no OS, such as `time::Duration`, `sync::atomic` and the `net`
//! address types.
//!
//! | Feature   | Provides                                                                                 |
//! |-----------|------------------------------------------------------------------------------------------|
//! | `alloc`   | `alloc` at std's paths: `Box`, `Vec`, `String`, `collections`, `sync::Arc`               |
//! | `command` | `process::{Command, Child, Output, ExitStatus, Stdio}`, `os::*::process`                 |
//! | `env`     | `env::{args, var, vars, current_dir, current_exe, temp_dir, home_dir, consts}`           |
//! | `fs`      | `fs` and `os::{unix, linux, macos, darwin, windows}::fs`                                 |
//! | `io`      | `Read`, `Write`, `Seek`, `BufRead` and co., `Error`, `pipe`, `os::fd`, `os::windows::io` |
//! | `net`     | `TcpStream`, `TcpListener`, `UdpSocket`, `ToSocketAddrs`, `os::unix::net`                |
//! | `path`    | `Path`, `PathBuf`, `OsStr`, `OsString` and `os::{unix, wasi, windows}::ffi`              |
//! | `process` | `process::exit`, `process::abort` and `process::id`                                      |
//! | `stdio`   | `print!` and co., `dbg!`; with `io`, the `io::stdin`, `stdout` and `stderr` handles      |
//! | `sync`    | `Mutex`, `RwLock`, `Condvar`, `Once`, `OnceLock`, `LazyLock` and `Barrier`               |
//! | `thread`  | `thread::{spawn, scope, park, sleep, current}`, `thread_local!`, `os::*::thread`         |
//! | `time`    | `Instant`, `SystemTime`, `UNIX_EPOCH` and `SystemTimeError`                              |
//! | `nightly` | Needs a nightly compiler: `thread_local!` keeps values in native thread-locals, like std |
//!
//! `io` and `path` need `alloc`; `net` and `thread` need `alloc` and `io`;
//! `env` needs `alloc`, `io` and `path`; `fs` needs `alloc`, `io`, `path` and
//! `time`; `command` needs `alloc`, `io`, `path` and `process`. The standard
//! stream handles need `io` and `stdio`. Without `alloc`, litestd does not
//! link the `alloc` crate, so a binary needs no global allocator.
//!
//! `nightly` changes no API. Without it, `thread_local!` finds each value
//! through one OS key and a per-thread table; with it, each static is a
//! `#[thread_local]` static of its own, as in std, which makes an access a
//! few instructions instead of a call into the C library or the system.
//!
//! # std and litestd are mutually exclusive
//!
//! litestd defines the `#[panic_handler]`, and so does std, so a crate graph
//! that contains both does not compile:
//!
//! ```text
//! error[E0152]: duplicate lang item in crate `litestd` (which `app` depends
//! on): `panic_impl`
//! ```
//!
//! This means litestd is used where std is present: use std there, for
//! example through the `std` feature of the crate that switched. Test
//! binaries always link std, so the tests of a crate that uses litestd
//! enable its `test-with-std` feature, typically from `[dev-dependencies]`,
//! which compiles the handler out.
//!
//! A binary with its own `#[panic_handler]` enables `custom-panic-handler`,
//! which compiles litestd's out; if it also links std, it fails on its own
//! handler. A library that enabled `custom-panic-handler` or
//! `test-with-std` would silence the check for every program using it, so
//! libraries must never enable either.
//!
//! # Binaries
//!
//! A `no_std` binary is built with `panic = "abort"` and exports the C
//! entry point, which wasi-libc calls `__main_argc_argv`. It enables
//! `global-allocator` to register
//! [`alloc::System`] as its global allocator, unless it brings its own, and
//! optionally `panic-location`, which makes the panic handler print the
//! panic location to stderr without linking the formatting machinery, or
//! `panic-message`, which prints the location and the message, as std
//! does. The latter keeps every panic's message in the program, with what
//! it formats, such as the `Debug` of the errors that `unwrap` reports,
//! which costs from a few KiB to tens of KiB.
//!
//! ```ignore
//! #![no_std]
//! #![no_main]
//!
//! #[macro_use]
//! extern crate litestd as std;
//!
//! use std::ffi::{c_char, c_int};
//!
//! #[unsafe(no_mangle)]
//! extern "C" fn main(_argc: c_int, _argv: *const *const c_char) -> c_int {
//!     println!("Hello, world!");
//!     0
//! }
//! ```
//!
//! On Unix, with the `io` or `stdio` feature, litestd does the startup work
//! of std's runtime from an initializer that runs before `main`: it opens
//! `/dev/null` on the standard descriptors that are closed, and ignores
//! `SIGPIPE`, so that a write to a pipe or socket without a reader fails with
//! `BrokenPipe`. A program that would rather end on `SIGPIPE`, as C programs
//! do when their output goes away, restores its default action in `main`. A
//! shared library built on litestd does the same work in the process that
//! loads it. Unlike under std, a stack overflow is a plain fault. The
//! precompiled `core` and `alloc` refer to the unwinder even under `panic =
//! "abort"`, so every `no_std` binary outside WebAssembly gets an aborting
//! `rust_eh_personality`, which it must not define itself, and the
//! platform's unwinder library. Static
//! musl binaries on `x86_64` and `aarch64` get a weak aborting `_Unwind_Resume`
//! instead, which saves about 80 KiB and yields to a real unwinder if one is
//! linked.

#![no_std]
#![cfg_attr(docsrs, feature(doc_cfg))]
// `thread_local!` expands to `#[thread_local]` statics and an `unsafe`
// block, as std's does, in crates that neither enable the feature nor allow
// `unsafe`; only these internal features make that possible.
#![cfg_attr(
    all(feature = "nightly", feature = "thread"),
    feature(allow_internal_unsafe, allow_internal_unstable),
    allow(internal_features)
)]
// WebAssembly threads block on `memory.atomic.wait32`, and keep per-thread
// state in `#[thread_local]` statics.
#![cfg_attr(
    all(
        feature = "nightly",
        target_family = "wasm",
        target_feature = "atomics",
        any(
            all(target_os = "wasi", feature = "env"),
            feature = "stdio",
            feature = "sync",
            feature = "thread"
        )
    ),
    feature(stdarch_wasm_atomic_wait)
)]
#![cfg_attr(
    all(
        feature = "nightly",
        any(
            feature = "thread",
            all(target_os = "unknown", target_feature = "atomics")
        )
    ),
    feature(thread_local)
)]
// `sys` items are `pub(crate)` so that `unreachable_pub` can check the
// public API; clippy's `redundant_pub_crate` asks for the opposite.
#![allow(clippy::redundant_pub_crate)]

#[cfg(feature = "alloc")]
extern crate alloc as alloc_crate;

pub mod alloc;
#[cfg(feature = "env")]
pub mod env;
pub mod ffi;
#[cfg(feature = "fs")]
pub mod fs;
#[cfg(feature = "io")]
pub mod io;
pub mod net;
pub mod os;
#[cfg(feature = "path")]
pub mod path;
pub mod prelude;
#[cfg(feature = "process")]
pub mod process;
pub mod sync;
#[cfg(feature = "thread")]
pub mod thread;
pub mod time;

mod stdio;
mod sys;
#[cfg(any(
    all(unix, feature = "command"),
    feature = "sync",
    feature = "thread"
))]
mod unwind;

// What std re-exports from `core` beyond the minimum supported compiler,
// where the compiler has it.
#[cfg(litestd_core_1_95)]
pub use core::cfg_select;
pub use core::{
    any, arch, array, ascii, cell, char, clone, cmp, convert, default, error,
    f32, f64, future, hash, hint, iter, marker, mem, num, ops, option, panic,
    pin, primitive, ptr, result,
};
// The macros std re-exports from core, such as `std::write!`.
pub use core::{
    assert, assert_eq, assert_ne, cfg, column, compile_error, concat,
    debug_assert, debug_assert_eq, debug_assert_ne, env, file, format_args,
    include, include_bytes, include_str, line, matches, module_path,
    option_env, stringify, todo, unimplemented, unreachable, write, writeln,
};
#[cfg(litestd_core_1_96)]
pub use core::{assert_matches, debug_assert_matches, range};
// Without `alloc`, these modules fall back to their `core` parts.
#[cfg(not(feature = "alloc"))]
pub use core::{borrow, fmt, slice, str};

#[cfg(feature = "alloc")]
pub use alloc_crate::{
    borrow, boxed, fmt, format, rc, slice, str, string, vec,
};
// The functions behind the print macros, which name them through `$crate`.
#[cfg(feature = "stdio")]
#[doc(hidden)]
pub use stdio::{_eprint, _eprintln, _print, _println};

#[cfg(feature = "alloc")]
pub mod collections {
    //! Collection types. std's `HashMap` and `HashSet` need a randomly
    //! seeded hasher and are not provided.

    pub use core::ops::Bound;

    pub use alloc_crate::collections::*;
}

pub mod task {
    //! Types and traits for working with asynchronous tasks.

    pub use core::task::*;

    #[cfg(feature = "alloc")]
    pub use alloc_crate::task::*;
}
