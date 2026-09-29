//! A `no_std` program that panics, to show the panic handler.
//!
//! Built with `--features global-allocator`, it aborts without a word. Built
//! with `--features global-allocator,panic-location`, it first prints where it
//! panicked, as `panicked at examples/panic.rs:LINE:COLUMN`; with
//! `panic-message` instead of `panic-location`, it prints that, a colon, and
//! the message on the next line, as std does.

#![no_std]
#![no_main]
// Written against std's paths on purpose: that is what the switch provides.
#![allow(clippy::std_instead_of_core, clippy::std_instead_of_alloc)]
// Panicking is the point of this program.
#![allow(clippy::panic)]

#[macro_use]
extern crate litestd as std;

use std::ffi::{c_char, c_int};

/// The C runtime's entry point, which wasi-libc calls `__main_argc_argv`.
#[cfg_attr(not(target_os = "wasi"), unsafe(no_mangle))]
#[cfg_attr(target_os = "wasi", unsafe(export_name = "__main_argc_argv"))]
extern "C" fn main(argc: c_int, _argv: *const *const c_char) -> c_int {
    panic!("the example panicked with argc = {argc}")
}
