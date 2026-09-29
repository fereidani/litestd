//! Writes, reads, lists and removes a file in its own temporary directory.

#![cfg_attr(feature = "lite", no_std, no_main)]

#[cfg(feature = "lite")]
#[macro_use]
extern crate litestd as std;

use std::{env, fs, prelude::rust_2024::*, process};

size_probe::main! {
    let dir = env::temp_dir().join(format!("litestd-size-{}", process::id()));
    fs::create_dir(&dir).unwrap();
    let file = dir.join("probe.txt");
    fs::write(&file, b"size probe").unwrap();
    assert!(fs::read(&file).unwrap() == b"size probe");
    assert!(fs::metadata(&file).unwrap().len() == 10);
    assert!(fs::read_dir(&dir).unwrap().count() == 1);
    fs::remove_file(&file).unwrap();
    fs::remove_dir(&dir).unwrap();
    println!("fs done in {}", dir.display());
}
