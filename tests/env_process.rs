//! `litestd::env::args` and `args_os` against std's, in children of this
//! program started with chosen arguments, and `set_var` racing readers.
//!
//! This test has its own `main`: libtest reads the arguments itself and
//! rejects the unusual ones passed here. Under Miri, which cannot spawn
//! processes, only the race runs, in this process, which runs nothing else.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    reason = "helpers, like tests, fail on errors"
)]

#[cfg(unix)]
use std::os::unix::ffi as std_ffi;
#[cfg(target_os = "wasi")]
use std::os::wasi::ffi as std_ffi;
use std::{ffi::OsString, process::Command};

use litestd::env as lenv;

/// Set in the children, naming what they check.
const MODE: &str = "LITESTD_ENV_PROCESS_MODE";

/// Set in the children, to the number of arguments passed after the
/// program's.
const PASSED: &str = "LITESTD_ENV_PROCESS_PASSED";

fn main() {
    if cfg!(miri) {
        concurrent_changes(5);
        return;
    }
    // WebAssembly has no child processes or threads: compare this process's
    // own arguments, which the WASI runner passes on.
    if cfg!(target_family = "wasm") {
        compare();
        println!("env_process: all checks passed");
        return;
    }
    match std::env::var(MODE).as_deref() {
        Ok("compare") => compare(),
        Ok("args-panics") => lenv::args().for_each(drop),
        Ok("concurrent") => concurrent_changes(5000),
        Ok(other) => panic!("unknown mode {other}"),
        Err(_) => parent(),
    }
}

/// The units of a litestd OS string: bytes off Windows, UTF-16 on Windows.
fn lite_units(s: &litestd::ffi::OsStr) -> Vec<u32> {
    #[cfg(not(windows))]
    let units = s.as_encoded_bytes().iter().map(|&b| u32::from(b)).collect();
    #[cfg(windows)]
    let units = litestd::os::windows::ffi::OsStrExt::encode_wide(s)
        .map(u32::from)
        .collect();
    units
}

/// The units of a std OS string, as [`lite_units`] gives them.
fn std_units(s: &std::ffi::OsStr) -> Vec<u32> {
    #[cfg(not(windows))]
    let units = s.as_encoded_bytes().iter().map(|&b| u32::from(b)).collect();
    #[cfg(windows)]
    let units = std::os::windows::ffi::OsStrExt::encode_wide(s)
        .map(u32::from)
        .collect();
    units
}

/// The child side: litestd's arguments, taken every way, against std's.
fn compare() {
    let real: Vec<OsString> = std::env::args_os().collect();
    if let Ok(passed) = std::env::var(PASSED) {
        assert_eq!(real.len(), passed.parse::<usize>().unwrap() + 1);
    }
    let real: Vec<_> = real.iter().map(|arg| std_units(arg)).collect();
    let lite: Vec<_> = lenv::args_os().map(|arg| lite_units(&arg)).collect();
    assert_eq!(lite, real);
    let rev: Vec<_> = lenv::args_os().rev().map(|a| lite_units(&a)).collect();
    assert!(rev.iter().eq(real.iter().rev()));
    assert_eq!(lenv::args_os().count(), real.len());
    assert_eq!(
        lenv::args_os().last().map(|a| lite_units(&a)).as_ref(),
        real.last()
    );
    for n in 0..real.len() + 2 {
        let nth = lenv::args_os().nth(n).map(|a| lite_units(&a));
        assert_eq!(nth.as_ref(), real.get(n), "nth({n})");
    }
    // Takes from both ends in turn, checking what is left at each step.
    let (mut lite, mut real_iter) = (lenv::args_os(), std::env::args_os());
    for step in 0..=real.len() {
        assert_eq!(lite.len(), real_iter.len());
        assert_eq!(lite.size_hint(), real_iter.size_hint());
        assert_eq!(format!("{lite:?}"), format!("{real_iter:?}"));
        let (a, b) = if step % 2 == 0 {
            (lite.next(), real_iter.next())
        } else {
            (lite.next_back(), real_iter.next_back())
        };
        assert_eq!(a.map(|a| lite_units(&a)), b.map(|b| std_units(&b)));
    }
    assert!(lite.next().is_none() && lite.next_back().is_none());
    if std::env::args_os().all(|arg| arg.to_str().is_some()) {
        let lite: Vec<String> = lenv::args().collect();
        assert_eq!(lite, std::env::args().collect::<Vec<_>>());
        let (lite, real) = (lenv::args(), std::env::args());
        assert_eq!(format!("{lite:?}"), format!("{real:?}"));
        assert_eq!(lite.len(), real.len());
        assert_eq!(lenv::args().next_back(), std::env::args().next_back());
        let (mut lite, mut real) = (lenv::args(), std::env::args());
        for step in 0..=lite.len() {
            assert_eq!(lite.len(), real.len());
            if step % 2 == 0 {
                assert_eq!(lite.next(), real.next());
            } else {
                assert_eq!(lite.next_back(), real.next_back());
            }
        }
    }
}

/// Starts this program with `args` in mode `mode`, and returns its output.
fn run(mode: &str, setup: impl FnOnce(&mut Command)) -> std::process::Output {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.env(MODE, mode);
    setup(&mut command);
    command.output().unwrap()
}

/// Asserts that the child `run` returned passed its check.
fn passed(what: &str, out: &std::process::Output) {
    assert!(
        out.status.success(),
        "{what}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Compares the arguments of a child started with `args`.
fn compare_args<S: AsRef<std::ffi::OsStr>>(args: &[S]) {
    let out = run("compare", |command| {
        command.args(args).env(PASSED, args.len().to_string());
    });
    let shown: Vec<_> = args.iter().map(AsRef::as_ref).take(8).collect();
    passed(&format!("{shown:?}"), &out);
}

fn parent() {
    #[cfg(unix)]
    forbid_core_files();
    compare_args::<&str>(&[]);
    compare_args(&["a"]);
    compare_args(&[
        "",
        "b c",
        "\t",
        " ",
        "caf\u{e9} \u{1f600}",
        "--flag",
        "-",
        "a=b",
        "\"q\"",
        "\\",
        "a\\\\\"b",
        "\\\\",
        "x\\",
    ]);
    // Windows limits a command line to 32767 units.
    compare_args(&["x".repeat(if cfg!(windows) { 30_000 } else { 100_000 })]);
    let many: Vec<String> = (0..3000).map(|i| i.to_string()).collect();
    compare_args(&many);
    #[cfg(unix)]
    unix_args();
    #[cfg(windows)]
    windows_args();
    #[cfg(not(target_os = "unknown"))]
    args_reject_invalid();
    passed("concurrent changes", &run("concurrent", |_| {}));
    println!("env_process: all checks passed");
}

/// Checks that `args()` panics on an argument that is not Unicode, which
/// wasm32-unknown-unknown can neither make nor pass to a child.
#[cfg(not(target_os = "unknown"))]
fn args_reject_invalid() {
    let out = run("args-panics", |command| {
        command.arg(not_unicode());
    });
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "args() accepted {:?}", not_unicode());
    assert!(stderr.contains("not valid Unicode"), "{stderr}");
}

/// An argument that is not Unicode.
#[cfg(not(target_os = "unknown"))]
fn not_unicode() -> OsString {
    #[cfg(not(windows))]
    let arg = crate::std_ffi::OsStringExt::from_vec(b"a\xffb".to_vec());
    #[cfg(windows)]
    let arg = std::os::windows::ffi::OsStringExt::from_wide(&[0x61, 0xD800]);
    arg
}

/// Keeps the children, some of which panic on purpose, from writing core
/// files. The children inherit this process's limit.
#[cfg(unix)]
fn forbid_core_files() {
    let limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a valid `rlimit` for the duration of the call.
    let r = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &raw const limit) };
    assert_eq!(r, 0);
}

#[cfg(unix)]
fn unix_args() {
    use crate::std_ffi::OsStrExt;
    let odd = |bytes: &'static [u8]| std::ffi::OsStr::from_bytes(bytes);
    compare_args(&[odd(b"\xff\xfe"), odd(b"x\x80y"), odd(b""), odd(b"\xc3")]);
}

#[cfg(windows)]
fn windows_args() {
    use std::os::windows::{ffi::OsStringExt, process::CommandExt};
    compare_args(&[
        OsString::from_wide(&[0xD800]),
        OsString::from_wide(&[0x61, 0xDC00, 0x62]),
        OsString::from_wide(&[0xD83D, 0xDE00]),
    ]);
    // Command lines after the program's name, taken literally, from the
    // rules of the C runtime and std's tests of them.
    for raw in [
        r#""abc" d e"#,
        r#"a\\\b d"e f"g h"#,
        r#"a\\\"b c d"#,
        r#"a\\\\"b c" d e"#,
        r#""" """#,
        r#""" """"#,
        r#""this is """all""" in the same argument""#,
        r#""a"""#,
        r#""a"" a"#,
        r#"Cal"l Me I"shmael"#,
        r#"CallMe\"Ishmael"#,
        r#""CallMe\"Ishmael""#,
        r#""Call Me Ishmael\\""#,
        r#""CallMe\\\"Ishmael""#,
        r#""a\\\b""#,
        r#""\"Call Me Ishmael\"""#,
        r#""C:\TEST A\\""#,
        r#""\"C:\TEST A\\\"""#,
        r#""a b c"  d  e"#,
        r#""ab\"c"  "\\"  d"#,
        r#""a b c"""#,
        r#""""CallMeIshmael"""  b  c"#,
        r#""""Call Me Ishmael""""#,
        r#"""""Call Me Ishmael"" b c"#,
        "a\tb \t c  ",
        "\"unterminated",
        "trailing\\",
        "\"\"",
        "\"",
        "\\\"",
    ] {
        let out = run("compare", |command| {
            command.raw_arg(raw);
        });
        passed(raw, &out);
    }
    #[cfg(feature = "fs")]
    program_names();
}

/// Starts this program with exactly the command line `line`, which
/// `Command` cannot make: it always quotes the program's name.
#[cfg(all(windows, feature = "fs"))]
fn run_command_line(line: &str) -> u32 {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{
            CREATE_UNICODE_ENVIRONMENT, CreateProcessW, GetExitCodeProcess,
            INFINITE, PROCESS_INFORMATION, STARTUPINFOW, WaitForSingleObject,
        },
    };

    let exe = std::env::current_exe().unwrap();
    let exe: Vec<u16> = exe.as_os_str().encode_wide().chain([0]).collect();
    let mut cmd: Vec<u16> = line.encode_utf16().chain([0]).collect();
    let mut env = Vec::new();
    for (key, value) in std::env::vars_os() {
        env.extend(key.encode_wide().chain([u16::from(b'=')]));
        env.extend(value.encode_wide().chain([0]));
    }
    env.extend(format!("{MODE}=compare").encode_utf16().chain([0, 0]));
    // SAFETY: both structures are plain data, valid when zeroed.
    let mut startup: STARTUPINFOW = unsafe { core::mem::zeroed() };
    startup.cb = u32::try_from(size_of::<STARTUPINFOW>()).unwrap();
    // SAFETY: as above.
    let mut info: PROCESS_INFORMATION = unsafe { core::mem::zeroed() };
    // SAFETY: the strings are NUL-terminated, `cmd` is writable, `env` is a
    // UTF-16 environment block, and the structures are valid.
    let created = unsafe {
        CreateProcessW(
            exe.as_ptr(),
            cmd.as_mut_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            0,
            CREATE_UNICODE_ENVIRONMENT,
            env.as_ptr().cast(),
            core::ptr::null(),
            &raw const startup,
            &raw mut info,
        )
    };
    assert_ne!(created, 0, "{line:?}: {}", std::io::Error::last_os_error());
    let mut code = 0;
    // SAFETY: the handles are open until closed below.
    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        GetExitCodeProcess(info.hProcess, &raw mut code);
        CloseHandle(info.hProcess);
        CloseHandle(info.hThread);
    }
    code
}

/// The program's name, parsed without escapes, from std's tests.
#[cfg(all(windows, feature = "fs"))]
fn program_names() {
    for line in [
        "EXE check",
        r#""EXE" check"#,
        r#""EXE check""#,
        r#""EXE """for""" check"#,
        r#""EXE \"for\" check"#,
        r#""EXE \" for \" check"#,
        r#"E"X"E test"#,
        r#"EX""E test"#,
        " test",
        "  test test2 ",
        "\ttab",
        "",
    ] {
        assert_eq!(run_command_line(line), 0, "{line:?}");
    }
}

/// Reads the environment through litestd on several threads while another
/// sets and removes a variable, which the lock must keep apart.
fn concurrent_changes(rounds: usize) {
    const KEY: &str = "LITESTD_ENV_RACE";
    let long = "l".repeat(1000);
    let valid = |value: &litestd::ffi::OsStr| {
        value.to_str().is_some_and(|v| v == "a" || v == long)
    };
    std::thread::scope(|scope| {
        for _ in 0..3 {
            scope.spawn(|| {
                for _ in 0..rounds {
                    if let Some(value) = lenv::var_os(KEY) {
                        assert!(valid(&value));
                    }
                    for (key, value) in lenv::vars_os() {
                        assert!(key != *KEY || valid(&value));
                    }
                }
            });
        }
        scope.spawn(|| {
            for round in 0..rounds {
                // SAFETY: every thread of this process reads and changes
                // the environment through litestd only, while this runs.
                unsafe {
                    match round % 3 {
                        0 => lenv::set_var(KEY, "a"),
                        1 => lenv::set_var(KEY, &long),
                        _ => lenv::remove_var(KEY),
                    }
                }
            }
        });
    });
}
