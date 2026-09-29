//! The `print!` family of macros, checked on a child process's real
//! standard streams.
//!
//! The macros write to the OS streams directly, past the test harness's
//! output capture, so each test runs this binary again with `LITESTD_CHILD`
//! naming what the child prints. The child prints `MARK` first, so the
//! harness's own banner can be cut off, and exits through `process::exit`
//! before the harness prints anything else. Miri cannot spawn processes,
//! so under Miri the child's code runs in-process instead.

#![cfg(all(feature = "stdio", feature = "process"))]

#[cfg(not(miri))]
use std::process::{Command, Output};

const MARK: &str = "<<litestd>>\n";

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

/// The length of the long lines, well beyond the 1 KiB print buffer.
const LONG: usize = if cfg!(miri) { 1500 } else { 5000 };

/// Runs `child` in a new process with `LITESTD_CHILD=mode`, and returns
/// what it wrote after `MARK` to stdout and stderr.
#[cfg(test)]
#[cfg(not(miri))]
fn run_child(mode: &str) -> (String, String) {
    #[cfg(unix)]
    forbid_core_files();
    let exe = std::env::current_exe().unwrap();
    let out: Output = Command::new(exe)
        .args(["--exact", "child", "--nocapture", "--test-threads=1", "-q"])
        .env("LITESTD_CHILD", mode)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let after_mark = |bytes: Vec<u8>| {
        let text = String::from_utf8(bytes).unwrap();
        let (_, rest) = text.split_once(MARK).expect("child output marker");
        rest.to_owned()
    };
    (after_mark(out.stdout), after_mark(out.stderr))
}

/// The child side of every test; a no-op in the parent.
#[test]
fn child() {
    let Ok(mode) = std::env::var("LITESTD_CHILD") else {
        return;
    };
    litestd::print!("{MARK}");
    litestd::eprint!("{MARK}");
    print_mode(&mode);
    litestd::process::exit(0)
}

/// A struct for `dbg!` to pretty-print.
#[derive(Debug)]
#[allow(dead_code, reason = "only printed")]
struct Point {
    x: i32,
    y: i32,
}

/// Prints what the child mode `mode` prints.
#[cfg(test)]
fn print_mode(mode: &str) {
    match mode {
        "macros" => {
            let name = "litestd";
            litestd::print!("a");
            litestd::print!("b{}", 1);
            litestd::println!();
            litestd::println!("plain line");
            litestd::println!("{name} {} {:>5}|{:<3}|", 42, "r", 'l');
            litestd::println!("{0}-{0}", name);
            litestd::eprint!("e");
            litestd::eprintln!();
            litestd::eprintln!("err {name}");
            litestd::eprintln!("{:?}", Some("x"));
            litestd::println!("{}", "unicode \u{e9}\u{4e16}");
            // A trailing comma is accepted, as with std.
            litestd::println!("{}", 1,);
        }
        "dbg" => {
            let a = 2;
            let doubled = litestd::dbg!(a * 2);
            assert_eq!(doubled, 4);
            let (x, y) = litestd::dbg!(1u8, "two");
            assert_eq!((x, y), (1, "two"));
            let moved = litestd::dbg!(String::from("owned"));
            assert_eq!(moved, "owned");
            litestd::dbg!();
            litestd::dbg!(Point { x: 1, y: -1 });
        }
        "long" => {
            let long = "x".repeat(LONG);
            litestd::println!("{long}");
            litestd::print!("{}", long);
            litestd::println!("|{}|", &long[..10]);
            litestd::eprintln!("{long}");
            litestd::println!("{}", "y".repeat(1023));
            litestd::println!("{}", "z".repeat(1024));
        }
        "threads" => {
            std::thread::scope(|s| {
                for t in 0..8u8 {
                    s.spawn(move || {
                        let fill = char::from(b'a' + t);
                        for i in 0..200 {
                            let body: String =
                                core::iter::repeat_n(fill, 90).collect();
                            litestd::println!("{t}:{i:03}:{body}");
                        }
                    });
                }
            });
        }
        "until_closed" => {
            // Far more than a pipe holds: the writes fail once the parent
            // closes its end.
            for i in 0..100_000 {
                litestd::println!("{i:0>90}");
            }
        }
        other => panic!("unknown child mode {other}"),
    }
}

/// A failed print panics, as in std. Once the child printed `MARK`, the
/// parent closes its end of the child's stdout, so the child's writes fail
/// with `EPIPE`, as the child, a std program, ignores `SIGPIPE`, or with
/// `ERROR_NO_DATA` on Windows. std's own message for its harness's output,
/// which also fails then, adds the error to the text, so litestd's is told
/// apart by its exact line.
#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn failed_print_panics() {
    use std::{io::Read, process::Stdio};
    #[cfg(unix)]
    forbid_core_files();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child", "--nocapture", "--test-threads=1", "-q"])
        .env("LITESTD_CHILD", "until_closed")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut seen = Vec::new();
    let mut chunk = [0u8; 4096];
    while !String::from_utf8_lossy(&seen).contains(MARK) {
        let n = stdout.read(&mut chunk).unwrap();
        assert_ne!(n, 0, "child output marker");
        seen.extend_from_slice(&chunk[..n]);
    }
    // Closes the pipe; on WebAssembly, where the test is ignored, it cannot
    // exist.
    #[cfg_attr(target_family = "wasm", allow(clippy::drop_non_drop))]
    drop(stdout);
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success(), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr
            .lines()
            .any(|line| line == "failed printing to stdout"),
        "{stderr}"
    );
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn macros_write_exactly_what_std_would() {
    let (stdout, stderr) = run_child("macros");
    assert_eq!(
        stdout,
        "ab1\nplain line\nlitestd 42     r|l  |\nlitestd-litestd\n\
         unicode \u{e9}\u{4e16}\n1\n"
    );
    assert_eq!(stderr, "e\nerr litestd\nSome(\"x\")\n");
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn dbg_matches_std_format() {
    let (stdout, stderr) = run_child("dbg");
    assert_eq!(stdout, "");
    let file = file!();
    let lines: Vec<&str> = stderr.lines().collect();
    assert!(lines[0].starts_with(&format!("[{file}:")), "{stderr}");
    assert!(lines[0].ends_with("] a * 2 = 4"), "{stderr}");
    assert!(lines[1].ends_with("] 1u8 = 1"), "{stderr}");
    assert!(lines[2].ends_with("] \"two\" = \"two\""), "{stderr}");
    assert!(lines[3].ends_with("] String::from(\"owned\") = \"owned\""));
    assert!(
        lines[4].starts_with(&format!("[{file}:")) && lines[4].ends_with(']')
    );
    assert!(
        lines[5].ends_with("] Point { x: 1, y: -1 } = Point {"),
        "{stderr}"
    );
    assert_eq!(&lines[6..], ["    x: 1,", "    y: -1,", "}"]);
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn long_output_is_complete() {
    let (stdout, stderr) = run_child("long");
    let long = "x".repeat(LONG);
    let expected = format!(
        "{long}\n{long}|xxxxxxxxxx|\n{}\n{}\n",
        "y".repeat(1023),
        "z".repeat(1024)
    );
    assert_eq!(stdout, expected);
    assert_eq!(stderr, format!("{long}\n"));
}

/// Checks the lines of the `threads` child mode, each whole and on its
/// own, and returns how many each thread printed.
#[cfg(test)]
#[cfg(not(miri))]
fn count_thread_lines<'a>(lines: impl Iterator<Item = &'a str>) -> [usize; 8] {
    let mut counts = [0usize; 8];
    for line in lines {
        let mut parts = line.splitn(3, ':');
        let t: usize = parts.next().unwrap().parse().unwrap();
        let _index: usize = parts.next().unwrap().parse().unwrap();
        let body = parts.next().unwrap();
        let fill = char::from(b'a' + u8::try_from(t).unwrap());
        assert_eq!(body.len(), 90, "{line:?}");
        assert!(body.chars().all(|c| c == fill), "{line:?}");
        counts[t] += 1;
    }
    counts
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn lines_from_threads_do_not_interleave() {
    let (stdout, _) = run_child("threads");
    assert_eq!(count_thread_lines(stdout.lines()), [200; 8]);
}

/// A parent process may pass a standard handle opened for overlapped I/O,
/// on which a write can still be in flight when the call that started it
/// returns. Here the child's stdout is such a pipe, with a tiny buffer that
/// the parent drains late, so that the writes of the child's eight threads
/// wait for room in the pipe. Each write must return only once the kernel
/// is done with its buffer, and the child must print every line whole.
#[cfg(all(windows, not(miri)))]
#[test]
fn overlapped_stdout_receives_whole_lines() {
    use core::{ffi::c_void, ptr, time::Duration};
    use std::{
        fs::{File, OpenOptions},
        io::Read,
        os::windows::{fs::OpenOptionsExt, io::FromRawHandle},
        process::Stdio,
    };

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateNamedPipeW(
            name: *const u16,
            open_mode: u32,
            pipe_mode: u32,
            max_instances: u32,
            out_buffer_size: u32,
            in_buffer_size: u32,
            default_timeout: u32,
            security_attributes: *const c_void,
        ) -> *mut c_void;
    }
    const PIPE_ACCESS_INBOUND: u32 = 0x1;
    const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;

    let name = format!(r"\\.\pipe\litestd-overlapped-{}", std::process::id());
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 name, and the other
    // arguments are plain values; a byte-mode pipe with 64-byte buffers.
    let server = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_INBOUND,
            0,
            1,
            64,
            64,
            0,
            ptr::null(),
        )
    };
    assert!(!server.is_null() && server.addr() != usize::MAX, "{name}");
    // SAFETY: `server` is a fresh handle that nothing else owns.
    let mut server = unsafe { File::from_raw_handle(server) };
    let client = OpenOptions::new()
        .write(true)
        .custom_flags(FILE_FLAG_OVERLAPPED)
        .open(&name)
        .unwrap();
    // The command and its copy of `client` are dropped after the spawn, so
    // the pipe ends when the child does.
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child", "--nocapture", "--test-threads=1", "-q"])
        .env("LITESTD_CHILD", "threads")
        .stdout(client)
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let mut out = Vec::new();
    server.read_to_end(&mut out).unwrap();
    let status = child.wait().unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(status.success(), "{status:?}");
    let (_, rest) = text.split_once(MARK).expect("child output marker");
    assert_eq!(count_thread_lines(rest.lines()), [200; 8]);
}

/// Under Miri the child code runs in-process: the output is not checked,
/// but every path through the macros and the buffer runs under the
/// interpreter.
#[cfg(miri)]
#[test]
fn macros_run_under_miri() {
    for mode in ["macros", "dbg", "long"] {
        print_mode(mode);
    }
}
