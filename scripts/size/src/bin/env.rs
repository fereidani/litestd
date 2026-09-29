//! Reads the arguments and the environment variables.

#![cfg_attr(feature = "lite", no_std, no_main)]

#[cfg(feature = "lite")]
#[macro_use]
extern crate litestd as std;

use std::{env, prelude::rust_2024::*};

size_probe::main! {
    let args: Vec<String> = env::args().collect();
    let path = env::var("PATH").unwrap_or_default();
    let count = env::vars_os().count();
    assert!(!args.is_empty() && count > 0);
    println!("{} arguments, {count} variables, PATH of {}", args.len(), path.len());
}
