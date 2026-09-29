//! A `no_std` program that relies on the startup work litestd does before
//! `main`, as std's runtime does: on Unix, standard descriptors that are
//! closed are open again, on `/dev/null`, and `SIGPIPE` is ignored, so a
//! write to a pipe without a reader fails instead of ending the program.
//!
//! It prints its arguments, one per line, and exits with a bit set for each
//! check that fails: 1 if a standard descriptor is closed, 2 if a write to a
//! pipe without a reader does not fail with `BrokenPipe`. Where there are no
//! pipes, as on WebAssembly, it exits with 4.

#![no_std]
#![no_main]
// Written against std's paths on purpose: that is what the switch provides.
#![allow(clippy::std_instead_of_core, clippy::std_instead_of_alloc)]

#[macro_use]
extern crate litestd as std;

use std::{
    ffi::{c_char, c_int},
    io::{self, Write},
};

/// The C runtime's entry point, which wasi-libc calls `__main_argc_argv`.
#[cfg_attr(not(target_os = "wasi"), unsafe(no_mangle))]
#[cfg_attr(target_os = "wasi", unsafe(export_name = "__main_argc_argv"))]
extern "C" fn main(_argc: c_int, _argv: *const *const c_char) -> c_int {
    for arg in std::env::args().skip(1) {
        println!("{arg}");
    }
    let mut failed = 0;
    #[cfg(unix)]
    for fd in 0..3 {
        // SAFETY: `F_GETFD` reads the descriptor's flags; no memory is
        // involved.
        if unsafe { libc::fcntl(fd, libc::F_GETFD) } == -1 {
            failed |= 1;
        }
    }
    let Ok((reader, mut writer)) = io::pipe() else {
        return 4;
    };
    // wasm32-unknown-unknown has no pipes, so its `PipeReader` owns nothing.
    #[cfg_attr(target_os = "unknown", allow(clippy::drop_non_drop))]
    drop(reader);
    let written = writer.write(b"x");
    if !matches!(written, Err(e) if e.kind() == io::ErrorKind::BrokenPipe) {
        failed |= 2;
    }
    failed
}
