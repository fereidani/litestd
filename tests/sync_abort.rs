//! An initializer that unwinds aborts the process instead of leaving a
//! `Once`, `OnceLock` or `LazyLock` half-initialized. Each test runs itself
//! again in a child process, marked by an environment variable, where it
//! panics inside the initializer.
// WebAssembly has no child processes.
#![cfg(all(feature = "sync", not(target_family = "wasm")))]

use std::{
    env, io,
    process::{Command, Output},
};

use litestd::sync::{LazyLock, Once, OnceLock};

const CHILD: &str = "LITESTD_SYNC_ABORT_CHILD";
const MESSAGE: &str = "the initializer panics";

/// Runs the test `name` of this binary in a child process that takes the
/// panicking path.
fn run_child(name: &str) -> io::Result<Output> {
    Command::new(env::current_exe()?)
        .args(["--exact", name, "--test-threads=1", "--nocapture"])
        .env(CHILD, "1")
        .output()
}

/// Checks that the child panicked and then aborted.
fn assert_aborted(output: &Output) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(MESSAGE),
        "the child did not panic: {stderr}"
    );
    assert!(!output.status.success());
    // A panic that unwound out of the test would fail it with exit code 101.
    assert_ne!(
        output.status.code(),
        Some(101),
        "the panic unwound: {stderr}"
    );
}

fn in_child() -> bool {
    env::var_os(CHILD).is_some()
}

#[test]
#[cfg_attr(miri, ignore = "Miri cannot spawn processes")]
fn once() {
    if in_child() {
        Once::new().call_once(|| panic!("{MESSAGE}"));
        return;
    }
    assert_aborted(&run_child("once").unwrap());
}

#[test]
#[cfg_attr(miri, ignore = "Miri cannot spawn processes")]
fn once_force() {
    if in_child() {
        Once::new().call_once_force(|_| panic!("{MESSAGE}"));
        return;
    }
    assert_aborted(&run_child("once_force").unwrap());
}

#[test]
#[cfg_attr(miri, ignore = "Miri cannot spawn processes")]
fn once_lock() {
    if in_child() {
        let cell: OnceLock<i32> = OnceLock::new();
        cell.get_or_init(|| panic!("{MESSAGE}"));
        return;
    }
    assert_aborted(&run_child("once_lock").unwrap());
}

#[test]
#[cfg_attr(miri, ignore = "Miri cannot spawn processes")]
fn lazy_lock_force() {
    if in_child() {
        let lazy: LazyLock<i32> = LazyLock::new(|| panic!("{MESSAGE}"));
        let _ = LazyLock::force(&lazy);
        return;
    }
    assert_aborted(&run_child("lazy_lock_force").unwrap());
}

#[test]
#[cfg_attr(miri, ignore = "Miri cannot spawn processes")]
fn lazy_lock_force_mut() {
    if in_child() {
        let mut lazy: LazyLock<i32> = LazyLock::new(|| panic!("{MESSAGE}"));
        let _ = LazyLock::force_mut(&mut lazy);
        return;
    }
    assert_aborted(&run_child("lazy_lock_force_mut").unwrap());
}
