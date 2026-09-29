//! Tells litestd which items of `core` the compiler has beyond the minimum
//! supported version, so that litestd re-exports them where std does.

use std::{env, process::Command};

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!(
        "cargo::rustc-check-cfg=cfg(litestd_core_1_95, litestd_core_1_96)"
    );
    let rustc = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let Some(minor) = Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|version| minor_version(&version))
    else {
        return;
    };
    // `cfg_select!` is stable from 1.95, `assert_matches!`,
    // `debug_assert_matches!` and `core::range` from 1.96.
    if minor >= 95 {
        println!("cargo::rustc-cfg=litestd_core_1_95");
    }
    if minor >= 96 {
        println!("cargo::rustc-cfg=litestd_core_1_96");
    }
}

/// The minor version in `rustc 1.MINOR.PATCH ...`, nightlies included.
fn minor_version(version: &str) -> Option<u32> {
    let mut parts = version.split_whitespace().nth(1)?.split('.');
    if parts.next()? != "1" {
        return None;
    }
    parts.next()?.parse().ok()
}
