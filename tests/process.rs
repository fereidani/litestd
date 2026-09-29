//! `litestd::process`: the process id, `exit` and `abort`.
//!
//! `exit` and `abort` end the process, so those tests run this test binary
//! again with `LITESTD_CHILD` naming what the child does, and check how it
//! ended. Miri cannot spawn processes, so it runs only the in-process tests.

#![cfg(all(feature = "process", feature = "stdio"))]

#[cfg(not(miri))]
use std::process::{Command, Output};

/// Keeps the children that tests start, some of which abort on purpose,
/// from writing core files, which could land in the working tree. The
/// children inherit this process's limit.
#[cfg(all(test, unix, not(miri)))]
fn forbid_core_files() {
    let limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a valid `rlimit` for the duration of the call.
    let r = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &raw const limit) };
    assert_eq!(r, 0);
}

/// Runs `child` in a new process with `LITESTD_CHILD=mode`.
#[cfg(test)]
#[cfg(not(miri))]
fn run_child(mode: &str) -> Output {
    #[cfg(unix)]
    forbid_core_files();
    let exe = std::env::current_exe().unwrap();
    Command::new(exe)
        .args(["--exact", "child", "--nocapture", "--test-threads=1", "-q"])
        .env("LITESTD_CHILD", mode)
        .output()
        .unwrap()
}

/// The child side of the subprocess tests; a no-op in the parent.
#[test]
fn child() {
    let Ok(mode) = std::env::var("LITESTD_CHILD") else {
        return;
    };
    match mode.as_str() {
        "exit" => litestd::process::exit(42),
        "exit-wide" => litestd::process::exit(0x0100 + 7),
        "abort" => {
            litestd::print!("before abort");
            litestd::process::abort()
        }
        "exit-racing" => {
            // Every thread calls `exit` at once: exactly one runs the C
            // library's exit, the others wait for the process to end.
            let barrier = std::sync::Barrier::new(8);
            std::thread::scope(|s| {
                for _ in 0..8 {
                    s.spawn(|| {
                        barrier.wait();
                        litestd::process::exit(5)
                    });
                }
            });
            unreachable!("the process exits inside the scope")
        }
        #[cfg(unix)]
        "exit-handler" => {
            extern "C" fn handler() {
                litestd::print!("exit handler ran");
            }
            // SAFETY: `handler` is a valid function for the process's
            // lifetime.
            assert_eq!(unsafe { libc::atexit(handler) }, 0);
            litestd::process::exit(0)
        }
        #[cfg(unix)]
        "exit-reentrant" => {
            extern "C" fn handler() {
                litestd::process::exit(1)
            }
            // SAFETY: as above.
            assert_eq!(unsafe { libc::atexit(handler) }, 0);
            litestd::process::exit(0)
        }
        other => panic!("unknown child mode {other}"),
    }
}

#[test]
#[cfg_attr(target_family = "wasm", ignore = "std panics here on WebAssembly")]
fn id_matches_std() {
    assert_eq!(litestd::process::id(), std::process::id());
    assert_ne!(litestd::process::id(), 0);
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn exit_reports_the_code() {
    let out = run_child("exit");
    assert_eq!(out.status.code(), Some(42), "{out:?}");
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn exit_code_width_is_platform_specific() {
    let out = run_child("exit-wide");
    let expected = if cfg!(windows) { 0x0100 + 7 } else { 7 };
    assert_eq!(out.status.code(), Some(expected), "{out:?}");
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn abort_ends_abnormally_after_printing() {
    let out = run_child("abort");
    assert!(!out.status.success(), "{out:?}");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(out.status.signal(), Some(libc::SIGABRT), "{out:?}");
    }
    // Printing is unbuffered, so the text is out before the abort.
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.ends_with("before abort"), "{stdout:?}");
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn racing_exits_are_serialized() {
    for _ in 0..if cfg!(windows) { 3 } else { 20 } {
        let out = run_child("exit-racing");
        assert_eq!(out.status.code(), Some(5), "{out:?}");
    }
}

#[cfg(all(unix, not(miri)))]
#[test]
fn exit_runs_exit_handlers() {
    let out = run_child("exit-handler");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.ends_with("exit handler ran"), "{stdout:?}");
}

#[cfg(all(unix, not(miri)))]
#[test]
fn reentrant_exit_aborts() {
    use std::os::unix::process::ExitStatusExt;
    // C leaves a second `exit` from an exit handler undefined; litestd
    // aborts instead.
    let out = run_child("exit-reentrant");
    assert_eq!(out.status.signal(), Some(libc::SIGABRT), "{out:?}");
}
