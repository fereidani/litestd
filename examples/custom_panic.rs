//! A `no_std` program that brings its own panic handler.
//!
//! Build with `--features global-allocator,custom-panic-handler`, which
//! compiles litestd's handler out. The handler below prints a fixed message
//! and exits with code 101. litestd still provides the unwinder's
//! personality routine, so the program defines only the handler. Without
//! arguments it prints a line and exits normally; with any argument it
//! panics.

#![no_std]
#![no_main]
// Written against std's paths on purpose: that is what the switch provides.
#![allow(clippy::std_instead_of_core, clippy::std_instead_of_alloc)]

#[macro_use]
extern crate litestd as std;

use std::{
    ffi::{c_char, c_int},
    panic::PanicInfo,
    process,
};

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    eprintln!("custom panic handler");
    process::exit(101)
}

/// The C runtime's entry point, which wasi-libc calls `__main_argc_argv`.
#[cfg_attr(not(target_os = "wasi"), unsafe(no_mangle))]
#[cfg_attr(target_os = "wasi", unsafe(export_name = "__main_argc_argv"))]
extern "C" fn main(argc: c_int, _argv: *const *const c_char) -> c_int {
    assert!(argc <= 1, "the custom handler does not print this message");
    println!("no panic");
    0
}
