//! A `no_std` library written against std's API through litestd.

#![no_std]

#[macro_use]
extern crate litestd as std;

use std::{
    string::String,
    time::{Duration, Instant},
};

/// Returns how long `f` takes.
pub fn time_it(f: impl FnOnce()) -> Duration {
    let start = Instant::now();
    f();
    start.elapsed()
}

/// Prints a greeting and returns it.
pub fn greet(name: &str) -> String {
    let greeting = format!("hello {name}");
    println!("{greeting}");
    greeting
}
