//! Prints an OS error and a custom error with `Display` and `Debug`.

#![cfg_attr(feature = "lite", no_std, no_main)]

#[cfg(feature = "lite")]
#[macro_use]
extern crate litestd as std;

use std::io;

size_probe::main! {
    let os = io::Error::from_raw_os_error(2);
    let custom = io::Error::other("custom");
    println!("{os}\n{os:?}\n{custom}\n{custom:?}");
}
