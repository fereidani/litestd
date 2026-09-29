//! Prints a line.

#![cfg_attr(feature = "lite", no_std, no_main)]

#[cfg(feature = "lite")]
#[macro_use]
extern crate litestd as std;

size_probe::main! {
    println!("Hello, world!");
}
