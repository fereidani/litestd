//! Runs a child process and reads its output.

#![cfg_attr(feature = "lite", no_std, no_main)]

#[cfg(feature = "lite")]
#[macro_use]
extern crate litestd as std;

use std::process::Command;

size_probe::main! {
    let output = if cfg!(windows) {
        Command::new("cmd").args(["/C", "echo child"]).output()
    } else {
        Command::new("echo").arg("child").output()
    }
    .unwrap();
    assert!(output.status.success() && output.stdout.starts_with(b"child"));
    println!("{}", output.status);
}
