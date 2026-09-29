//! The panic handler, the mutual exclusion of std and litestd that it
//! enforces, and the linking of `no_std` binaries.
//!
//! A test binary links std, so litestd's handler is compiled out of it with
//! `test-with-std`. These tests therefore build separate crates with cargo,
//! each into a target directory of its own under this test's scratch
//! directory: the `panic` and `consumer` examples, to check how a `no_std`
//! binary ends when it panics, the `hello` example linked statically, and
//! the fixtures in `tests/fixtures`, to check that std and litestd cannot
//! be mixed. They need cargo, so they run under `cargo test` only, and not
//! under Miri.

#![cfg(not(miri))]

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

/// Returns the cargo running these tests, or `None` if the test binary was
/// started some other way, such as under wine.
fn cargo() -> Option<OsString> {
    let cargo = std::env::var_os("CARGO");
    if cargo.is_none() {
        eprintln!("CARGO is not set; skipping: run through `cargo test`");
    }
    cargo
}

/// The package root.
fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// A scratch directory for this test binary, under the target directory.
fn scratch(name: &str) -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join(name)
}

/// Keeps the children that tests start, some of which abort on purpose,
/// from writing core files, which could land in the working tree. The
/// children inherit this process's limit.
#[cfg(all(test, unix))]
fn forbid_core_files() {
    let limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a valid `rlimit` for the duration of the call.
    let r = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &raw const limit) };
    assert_eq!(r, 0);
}

/// Builds `example` with `features` and returns the program's path.
#[cfg(test)]
fn build_example(cargo: OsString, example: &str, features: &str) -> PathBuf {
    let target_dir = scratch(&features.replace(',', "-"));
    let status = Command::new(cargo)
        .current_dir(manifest_dir())
        .args(["build", "--quiet", "--offline", "--example", example])
        .args(["--features", features, "--target-dir"])
        .arg(&target_dir)
        .status()
        .unwrap();
    assert!(status.success(), "building {example} with {features}");
    #[cfg(unix)]
    forbid_core_files();
    target_dir
        .join("debug")
        .join("examples")
        .join(format!("{example}{}", std::env::consts::EXE_SUFFIX))
}

/// Builds `example` with `features` and runs it with `arg`, if not empty.
#[cfg(test)]
fn run_example(
    cargo: OsString,
    example: &str,
    features: &str,
    arg: &str,
) -> Output {
    let mut command = Command::new(build_example(cargo, example, features));
    if !arg.is_empty() {
        command.arg(arg);
    }
    command.output().unwrap()
}

/// Returns where the first `panic!` in `examples/{example}.rs` is, as
/// `Location` reports it: with the path that cargo gave the compiler, whose
/// separator is the platform's.
#[cfg(test)]
fn panic_site(example: &str) -> String {
    let path = Path::new("examples").join(format!("{example}.rs"));
    let source = fs::read_to_string(manifest_dir().join(&path)).unwrap();
    let (line, column) = source
        .lines()
        .enumerate()
        .find_map(|(i, text)| text.find("panic!(").map(|col| (i + 1, col + 1)))
        .unwrap();
    format!("{}:{line}:{column}", path.display())
}

/// Asserts that the process ended by aborting.
#[cfg(test)]
fn assert_aborted(out: &Output) {
    assert!(!out.status.success(), "{out:?}");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(out.status.signal(), Some(libc::SIGABRT), "{out:?}");
    }
}

/// Copies the fixture crate `name` to the scratch directory, pointing its
/// litestd dependency at this package, and runs `cargo check` on it.
#[cfg(test)]
fn check_fixture(cargo: OsString, name: &str) -> Output {
    let fixture = manifest_dir().join("tests").join("fixtures").join(name);
    let copy = scratch("fixtures").join(name);
    fs::create_dir_all(copy.join("src")).unwrap();
    let manifest = fs::read_to_string(fixture.join("Cargo.toml")).unwrap();
    let litestd = format!("path = '{}'", manifest_dir().display());
    assert!(manifest.contains("path = \"../../..\""), "{manifest}");
    fs::write(
        copy.join("Cargo.toml"),
        manifest.replace("path = \"../../..\"", &litestd),
    )
    .unwrap();
    for entry in fs::read_dir(fixture.join("src")).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), copy.join("src").join(entry.file_name()))
            .unwrap();
    }
    Command::new(cargo)
        .current_dir(&copy)
        .args(["check", "--offline", "--target-dir"])
        .arg(scratch("fixtures-target"))
        .output()
        .unwrap()
}

#[test]
fn panic_handler_aborts_silently() {
    let Some(cargo) = cargo() else { return };
    let out = run_example(cargo, "panic", "global-allocator", "");
    assert_aborted(&out);
    assert!(out.stdout.is_empty(), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
}

#[test]
fn panic_location_prints_where_and_aborts() {
    let Some(cargo) = cargo() else { return };
    let out =
        run_example(cargo, "panic", "global-allocator,panic-location", "");
    assert_aborted(&out);
    let stderr = String::from_utf8(out.stderr).unwrap();
    // The location only: the message is never printed.
    assert_eq!(stderr, format!("panicked at {}\n", panic_site("panic")));
}

#[test]
fn panic_message_prints_where_and_what_and_aborts() {
    let Some(cargo) = cargo() else { return };
    let out = run_example(cargo, "panic", "global-allocator,panic-message", "");
    assert_aborted(&out);
    let stderr = String::from_utf8(out.stderr).unwrap();
    let site = panic_site("panic");
    assert_eq!(
        stderr,
        format!("panicked at {site}:\nthe example panicked with argc = 1\n")
    );
}

/// litestd does std's startup work on Unix before `main`: the `startup`
/// example gets its arguments, and a write of its to a pipe without a
/// reader fails instead of raising `SIGPIPE`. Started with its standard
/// descriptors closed, it finds them open again.
#[cfg(unix)]
#[test]
fn startup_reopens_closed_streams_and_ignores_sigpipe() {
    use std::os::unix::process::CommandExt;
    let Some(cargo) = cargo() else { return };
    let exe = build_example(cargo, "startup", "global-allocator");
    let out = Command::new(&exe)
        .args(["first", "second one", ""])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(out.stdout, b"first\nsecond one\n\n");

    let mut command = Command::new(&exe);
    // SAFETY: the closure only calls `close`, which is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            for fd in 0..3 {
                libc::close(fd);
            }
            Ok(())
        });
    }
    let status = command.status().unwrap();
    assert_eq!(status.code(), Some(0), "{status:?}");
}

#[test]
fn consumer_example_runs_and_panics_with_location() {
    let Some(cargo) = cargo() else { return };
    let features = "global-allocator,panic-location";
    let out = run_example(cargo.clone(), "consumer", features, "");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("errors: "), "{stdout}");
    assert!(stdout.ends_with(" ok\n"), "{stdout}");

    let out = run_example(cargo.clone(), "consumer", features, "exit");
    assert_eq!(out.status.code(), Some(3), "{out:?}");

    let out = run_example(cargo, "consumer", features, "panic");
    assert_aborted(&out);
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(stderr, format!("panicked at {}\n", panic_site("consumer")));
}

#[test]
fn custom_panic_handler_replaces_litestd_handler() {
    let Some(cargo) = cargo() else { return };
    let features = "global-allocator,custom-panic-handler";
    let out = run_example(cargo.clone(), "custom_panic", features, "");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(out.stdout, b"no panic\n");

    let out = run_example(cargo, "custom_panic", features, "panic");
    assert_eq!(out.status.code(), Some(101), "{out:?}");
    assert_eq!(out.stderr, b"custom panic handler\n");
}

/// Returns whether the C compiler finds glibc's static C library.
#[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "gnu"))]
fn static_glibc_available() -> bool {
    let Ok(out) = Command::new("cc").arg("-print-file-name=libc.a").output()
    else {
        return false;
    };
    // The compiler echoes the bare name when it finds no such file.
    let path = String::from_utf8_lossy(&out.stdout);
    Path::new(path.trim()).is_absolute()
}

/// A static glibc binary links with GNU ld, which scans each static
/// archive once, in command-line order. glibc's `libc.a` and the unwinder
/// in `libgcc_eh.a` refer to each other, so litestd must list them in an
/// order that resolves both ways.
#[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "gnu"))]
#[test]
fn static_glibc_binary_links_with_gnu_ld() {
    let Some(cargo) = cargo() else { return };
    if !static_glibc_available() {
        eprintln!("no static glibc; skipping");
        return;
    }
    let target = "x86_64-unknown-linux-gnu";
    let target_dir = scratch("crt-static");
    let status = Command::new(cargo)
        .current_dir(manifest_dir())
        .args(["build", "--quiet", "--offline", "--example", "hello"])
        .args(["--features", "global-allocator", "--target", target])
        .arg("--target-dir")
        .arg(&target_dir)
        // The target links with LLD by default, which resolves symbols in
        // any order; GNU ld is the default everywhere else.
        .env(
            "RUSTFLAGS",
            "-C target-feature=+crt-static -C linker-features=-lld",
        )
        .status()
        .unwrap();
    assert!(status.success(), "linking hello statically with GNU ld");
    let exe = target_dir.join(target).join("debug/examples/hello");
    #[cfg(unix)]
    forbid_core_files();
    let out = Command::new(exe).output().unwrap();
    assert_eq!(out.stdout, b"Hello, world!\n", "{out:?}");
}

#[test]
fn std_and_litestd_do_not_mix() {
    let Some(cargo) = cargo() else { return };
    let out = check_fixture(cargo, "std_binary");
    assert!(!out.status.success(), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("duplicate lang item"), "{stderr}");
    assert!(stderr.contains("panic_impl"), "{stderr}");
}

#[test]
fn no_std_library_uses_litestd() {
    let Some(cargo) = cargo() else { return };
    let out = check_fixture(cargo, "no_std_library");
    assert!(out.status.success(), "{out:?}");
}

#[test]
fn custom_panic_handler_compiles_the_handler_out() {
    let Some(cargo) = cargo() else { return };
    // A std program that pulls in litestd with `custom-panic-handler` and
    // defines no handler type-checks: the gap that makes the feature
    // forbidden in libraries, and proof that it removes litestd's handler.
    let out = check_fixture(cargo, "std_binary_custom_handler");
    assert!(out.status.success(), "{out:?}");
}
