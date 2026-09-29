//! The smallest `no_std` program: prints one line through litestd.
//!
//! Build with `--features global-allocator`. The built-in panic handler
//! aborts without printing anything.

#![no_std]
#![no_main]
// Written against std's paths on purpose: that is what the switch provides.
#![allow(clippy::std_instead_of_core, clippy::std_instead_of_alloc)]

#[macro_use]
extern crate litestd as std;

use std::ffi::{c_char, c_int};

/// The C runtime's entry point, which wasi-libc calls `__main_argc_argv`.
#[cfg_attr(not(target_os = "wasi"), unsafe(no_mangle))]
#[cfg_attr(target_os = "wasi", unsafe(export_name = "__main_argc_argv"))]
extern "C" fn main(_argc: c_int, _argv: *const *const c_char) -> c_int {
    println!("Hello, world!");
    0
}
