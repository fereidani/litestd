//! The standard stream handles and `IsTerminal`, compared with std.
//!
//! The handles reach the OS streams directly, past the test harness's
//! output capture, so most tests run this binary again with `LITESTD_CHILD`
//! naming what the child does, often once with litestd and once with std,
//! and compare what the two printed. The child prints `MARK` first, so the
//! harness's banner can be cut off, and exits before the harness prints
//! anything else. Children whose streams are closed or null report through
//! their exit code instead. Miri cannot spawn processes, so under Miri the
//! in-process tests exercise the locks and buffers.

// wasm32-unknown-unknown has no streams to print to: `tests/unsupported.rs`
// compares its sinks with std's.
#![cfg(all(feature = "stdio", feature = "io", not(target_os = "unknown")))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "the child modes and helpers fail the test on errors"
)]
#![allow(
    clippy::explicit_write,
    clippy::print_in_format_impl,
    reason = "the tests write to the streams in every way"
)]

use core::fmt;

const MARK: &str = "<<litestd>>\n";

#[cfg(all(feature = "fs", not(miri)))]
#[test]
fn regular_files_are_not_terminals() {
    use litestd::io::IsTerminal;
    // WASI has no process ids or temporary directory in std; its runner
    // gives each program a `/tmp` of its own.
    let dir = if cfg!(target_os = "wasi") {
        std::path::PathBuf::from("/tmp/litestd-io-stdio")
    } else {
        std::env::temp_dir()
            .join(format!("litestd-io-stdio-{}", std::process::id()))
    };
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("file");
    let file = litestd::fs::File::create(path.to_str().unwrap()).unwrap();
    assert!(!file.is_terminal());
    assert!(!std::io::IsTerminal::is_terminal(
        &std::fs::File::open(&path).unwrap()
    ));
    #[cfg(unix)]
    {
        let fd = litestd::os::fd::OwnedFd::from(file);
        assert!(!litestd::os::fd::AsFd::as_fd(&fd).is_terminal());
        assert!(!fd.is_terminal());
    }
    #[cfg(windows)]
    {
        let handle = litestd::os::windows::io::OwnedHandle::from(file);
        let borrowed = litestd::os::windows::io::AsHandle::as_handle(&handle);
        assert!(!borrowed.is_terminal());
        assert!(!handle.is_terminal());
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A pseudo-terminal's secondary side is a terminal, and so is a child's
/// standard input when it is one.
#[cfg(all(unix, feature = "fs", not(miri)))]
#[test]
fn pseudo_terminals_are_terminals() {
    use litestd::io::IsTerminal;
    let Some((primary, secondary)) = open_pty() else {
        eprintln!("no pseudo-terminal available; skipping");
        return;
    };
    let file = litestd::fs::File::open(secondary.as_str()).unwrap();
    assert!(file.is_terminal());
    let fd = litestd::os::fd::OwnedFd::from(file);
    assert!(fd.is_terminal());
    let std_file = std::fs::File::open(&secondary).unwrap();
    let (stdout, _) =
        run_with("is_terminal", std::process::Stdio::from(std_file), None);
    // stdin is the terminal; stdout and stderr are pipes.
    let expected = "lite [true, false, false, true, false, false]\n\
                    std [true, false, false, true, false, false]\n";
    assert_eq!(stdout, expected);
    drop(primary);
}

#[cfg(all(
    unix,
    not(any(
        target_vendor = "apple",
        target_os = "openbsd",
        target_os = "dragonfly"
    )),
    feature = "fs",
    not(miri)
))]
use libc::ptsname_r;

/// OpenBSD and DragonFly lack `ptsname_r`: `ptsname`, whose buffer the
/// process shares, behind a lock stands in for it.
///
/// # Safety
///
/// `buf` must be valid for writes of `len` bytes.
#[cfg(all(
    any(target_os = "openbsd", target_os = "dragonfly"),
    feature = "fs",
    not(miri)
))]
unsafe fn ptsname_r(
    fd: libc::c_int,
    buf: *mut libc::c_char,
    len: libc::size_t,
) -> libc::c_int {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap();
    // SAFETY: the lock keeps other callers off the shared buffer until the
    // name is copied.
    let name = unsafe { libc::ptsname(fd) };
    if name.is_null() {
        return -1;
    }
    // SAFETY: `ptsname` returned a C string.
    let name = unsafe { core::ffi::CStr::from_ptr(name) }.to_bytes_with_nul();
    if name.len() > len {
        return -1;
    }
    // SAFETY: the caller vouches for `len` bytes at `buf`, and `name` fits.
    unsafe {
        core::ptr::copy_nonoverlapping(name.as_ptr(), buf.cast(), name.len());
    }
    0
}

// macOS has `ptsname_r` from 10.13.4, which the libc crate lacks there.
#[cfg(all(target_vendor = "apple", feature = "fs", not(miri)))]
unsafe extern "C" {
    fn ptsname_r(
        fd: libc::c_int,
        buf: *mut libc::c_char,
        len: libc::size_t,
    ) -> libc::c_int;
}

/// Opens a pseudo-terminal, returning its primary side and the path of its
/// secondary side, or `None` where the system has none to give.
#[cfg(all(unix, feature = "fs", not(miri)))]
fn open_pty() -> Option<(std::fs::File, String)> {
    use std::os::fd::FromRawFd;
    // SAFETY: `posix_openpt` has no preconditions.
    let fd = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) };
    if fd < 0 {
        return None;
    }
    // SAFETY: `fd` is a new descriptor that nothing else owns.
    let primary = unsafe { std::fs::File::from_raw_fd(fd) };
    let mut name = [0u8; 128];
    // SAFETY: `fd` is a pseudo-terminal primary; `name` is writable for its
    // length.
    let ok = unsafe {
        libc::grantpt(fd) == 0
            && libc::unlockpt(fd) == 0
            && ptsname_r(fd, name.as_mut_ptr().cast(), name.len()) == 0
    };
    if !ok {
        return None;
    }
    let name = core::ffi::CStr::from_bytes_until_nul(&name).ok()?;
    Some((primary, name.to_str().ok()?.to_owned()))
}

/// std's heuristic for msys and cygwin pseudo-terminals, which are named
/// pipes: litestd recognizes the same names.
#[cfg(all(windows, feature = "fs", not(miri)))]
#[test]
fn msys_pipes_are_terminals() {
    use litestd::io::IsTerminal;
    let id = std::process::id();
    for (name, expected) in [
        (format!("msys-{id}-pty0-to-master"), true),
        (format!("cygwin-{id}-pty1-from-master"), true),
        (format!("msys-{id}-pipe"), false),
        (format!("litestd-{id}-pty"), false),
    ] {
        let path = format!(r"\\.\pipe\{name}");
        let _servers = [0; 2]
            .map(|_| windows_pipe::create(&path, windows_pipe::INBOUND, 0));
        let client = litestd::fs::OpenOptions::new()
            .write(true)
            .open(path.as_str())
            .unwrap();
        let std_client =
            std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        assert_eq!(client.is_terminal(), expected, "{name}");
        assert_eq!(
            std::io::IsTerminal::is_terminal(&std_client),
            expected,
            "std: {name}"
        );
    }
}

/// Named pipes for the Windows tests.
#[cfg(all(windows, not(miri)))]
#[allow(
    clippy::redundant_pub_crate,
    reason = "`pub` would trip `unreachable_pub`"
)]
mod windows_pipe {
    use core::{ffi::c_void, ptr};
    use std::{fs::File, os::windows::io::FromRawHandle};

    pub(crate) const INBOUND: u32 = 0x1;
    pub(crate) const OUTBOUND: u32 = 0x2;
    pub(crate) const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;

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

    /// Creates a server end of a byte-mode pipe named `path`, which takes
    /// up to two, with buffers of `buffer` bytes.
    pub(crate) fn create(path: &str, access: u32, buffer: u32) -> File {
        let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
        // SAFETY: `wide` is a NUL-terminated UTF-16 name, and the other
        // arguments are plain values.
        let server = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                access,
                0,
                2,
                buffer,
                buffer,
                0,
                ptr::null(),
            )
        };
        assert!(!server.is_null() && server.addr() != usize::MAX, "{path}");
        // SAFETY: `server` is a fresh handle that nothing else owns.
        unsafe { File::from_raw_handle(server) }
    }
}

/// Under Miri the child code that needs no input runs in-process: its
/// output is not checked, but the locks, the reentrancy and the buffers run
/// under the interpreter.
#[cfg(miri)]
#[test]
fn handles_run_under_miri() {
    for mode in ["order_lite", "reentrant_lite", "write_fmt_error"] {
        child_mode(mode);
    }
    // Contention between threads, on the futex.
    std::thread::scope(|s| {
        for t in 0..3 {
            s.spawn(move || {
                for i in 0..3 {
                    let mut lock = litestd::io::stdout().lock();
                    litestd::io::Write::write_all(&mut lock, b"").unwrap();
                    litestd::print!("{}", "");
                    let _ = (t, i);
                }
            });
        }
    });
}

/// The child side of every test; a no-op in the parent.
#[test]
fn child() {
    let Ok(mode) = std::env::var("LITESTD_CHILD") else {
        return;
    };
    litestd::print!("{MARK}");
    litestd::eprint!("{MARK}");
    let code = child_mode(&mode);
    std::process::exit(code)
}

/// Runs the child mode `mode`, and returns its exit code.
fn child_mode(mode: &str) -> i32 {
    match mode {
        "read_line_lite" | "read_line_std" => {
            child_read_line(mode.ends_with("lite"));
        }
        "lines_lite" | "lines_std" => child_lines(mode.ends_with("lite")),
        "read_apis_lite" => child_read_apis_lite(),
        "read_apis_std" => child_read_apis_std(),
        "read_to_end" => {
            let mut all = Vec::new();
            let n = litestd::io::Read::read_to_end(
                &mut litestd::io::stdin(),
                &mut all,
            )
            .unwrap();
            litestd::println!("{n} {}", checksum(&all));
        }
        "order_lite" => child_order_lite(),
        "order_std" => child_order_std(),
        "reentrant_lite" => child_reentrant_lite(),
        "reentrant_std" => child_reentrant_std(),
        "exclusive" => child_exclusive(),
        "threads" => child_threads(),
        "write_fmt_error" => child_write_fmt_error(),
        "print_fmt_error" => child_print_fmt_error(),
        "is_terminal" => child_is_terminal(),
        "null" => return child_null(),
        "closed" => return child_closed(false),
        "invalid" => return child_closed(true),
        #[cfg(windows)]
        "console" => child_console(),
        #[cfg(windows)]
        "raw_bytes" => child_raw_bytes(),
        other => panic!("unknown child mode {other}"),
    }
    0
}

/// A cheap checksum, to compare long input with what was read.
fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0u64, |sum, &b| {
        sum.wrapping_mul(31).wrapping_add(u64::from(b))
    })
}

/// Echoes every line that `read_line` returns, with its count.
fn child_read_line(ours: bool) {
    let mut line = String::new();
    // At most one pass per line, and the input is finite.
    for _ in 0..100_000 {
        line.clear();
        let result = if ours {
            litestd::io::stdin()
                .read_line(&mut line)
                .map_err(|e| e.kind().to_string())
        } else {
            std::io::stdin()
                .read_line(&mut line)
                .map_err(|e| e.kind().to_string())
        };
        match result {
            Ok(0) => return,
            Ok(n) => litestd::println!("{n} {line:?}"),
            Err(kind) => litestd::println!("error {kind} {line:?}"),
        }
    }
}

/// Echoes every item of `lines`.
fn child_lines(ours: bool) {
    let print = |item: Result<String, String>| match item {
        Ok(line) => litestd::println!("{line:?}"),
        Err(kind) => litestd::println!("error {kind}"),
    };
    if ours {
        for item in litestd::io::stdin().lines() {
            print(item.map_err(|e| e.kind().to_string()));
        }
    } else {
        for item in std::io::stdin().lines() {
            print(item.map_err(|e| e.kind().to_string()));
        }
    }
}

/// Runs the reading methods of `Stdin` and `StdinLock` in turn, on input
/// that fits in the buffer, so that each result is deterministic.
macro_rules! read_apis {
    ($io:ident) => {{
        use $io::{BufRead, Read};
        let mut stdin = $io::stdin();
        let mut out = Vec::<String>::new();
        let mut five = [0u8; 5];
        stdin.read_exact(&mut five).unwrap();
        out.push(format!("exact {five:?}"));
        let mut lock = stdin.lock();
        {
            let first = lock.fill_buf().unwrap()[0];
            lock.consume(1);
            out.push(format!("fill {first}"));
            let mut until = Vec::new();
            let n = lock.read_until(b';', &mut until).unwrap();
            out.push(format!("until {n} {until:?}"));
            let n = lock.skip_until(b';').unwrap();
            out.push(format!("skip {n}"));
            let mut line = String::new();
            let n = lock.read_line(&mut line).unwrap();
            out.push(format!("line {n} {line:?}"));
            let bytes: Vec<u8> =
                lock.by_ref().bytes().take(3).map(Result::unwrap).collect();
            out.push(format!("bytes {bytes:?}"));
            let split = lock.by_ref().split(b',').next().unwrap().unwrap();
            drop(lock);
            out.push(format!("split {split:?}"));
        }
        let mut three = [0u8; 3];
        let n = (&stdin).read(&mut three).unwrap();
        out.push(format!("read {n} {three:?}"));
        let mut vectored = [0u8; 2];
        let mut more = [0u8; 2];
        let n = stdin
            .read_vectored(&mut [
                $io::IoSliceMut::new(&mut vectored),
                $io::IoSliceMut::new(&mut more),
            ])
            .unwrap();
        out.push(format!("vectored {n} {vectored:?} {more:?}"));
        let mut rest = String::new();
        let n = stdin.read_to_string(&mut rest).unwrap();
        out.push(format!("rest {n} {rest:?}"));
        let n = stdin.read(&mut three).unwrap();
        out.push(format!("eof {n}"));
        out
    }};
}

fn child_read_apis_lite() {
    for line in read_apis!(litestd_io) {
        litestd::println!("{line}");
    }
}

fn child_read_apis_std() {
    for line in read_apis!(std_io) {
        litestd::println!("{line}");
    }
}

use std::io as std_io;

use litestd::io as litestd_io;

/// Mixes every way of writing to stdout and stderr on one thread.
macro_rules! order {
    ($io:ident, $print:ident, $println:ident, $eprint:ident) => {{
        use $io::Write;
        for i in 0..40 {
            $print!("a{i}");
            $io::stdout().write_all(b"b").unwrap();
            $println!("c");
            writeln!($io::stdout(), "d{}", i).unwrap();
            $io::stdout().lock().write_all(b"e\n").unwrap();
            let mut out = $io::stdout();
            assert_eq!(out.write(b"f").unwrap(), 1);
            out.flush().unwrap();
            (&$io::stdout()).write_all(b"g\n").unwrap();
            $eprint!("x{i}");
            $io::stderr().write_all(b"y").unwrap();
            writeln!($io::stderr().lock(), "z").unwrap();
        }
    }};
}

fn child_order_lite() {
    use litestd::{eprint, print, println};
    order!(litestd_io, print, println, eprint);
}

fn child_order_std() {
    order!(std_io, print, println, eprint);
}

/// Prints from inside its `Display` implementation, through litestd.
struct NestedLite;

impl fmt::Display for NestedLite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use litestd::io::Write;
        f.write_str("<")?;
        litestd::print!("inner");
        litestd::io::stdout().write_all(b"[w]").unwrap();
        litestd::eprint!("[e]");
        f.write_str(">")
    }
}

/// Prints from inside its `Display` implementation, through std.
struct NestedStd;

impl fmt::Display for NestedStd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use std::io::Write;
        f.write_str("<")?;
        print!("inner");
        std::io::stdout().write_all(b"[w]").unwrap();
        eprint!("[e]");
        f.write_str(">")
    }
}

/// Prints while holding the locks, and from formatting code.
macro_rules! reentrant {
    ($io:ident, $nested:expr, $print:ident, $println:ident, $eprint:ident,
     $eprintln:ident) => {{
        use $io::Write;
        let out = $io::stdout();
        let mut lock = out.lock();
        $print!("1");
        write!(lock, "2").unwrap();
        $println!("3");
        writeln!(lock, "{}", $nested).unwrap();
        {
            let mut again = out.lock();
            again.write_all(b"4\n").unwrap();
        }
        $eprint!("e1 ");
        let mut err = $io::stderr().lock();
        $eprintln!("e2");
        writeln!(err, "e3 {}", $nested).unwrap();
        drop(err);
        lock.flush().unwrap();
        drop(lock);
        $println!("x{}y", $nested);
        $eprintln!("done");
    }};
}

fn child_reentrant_lite() {
    use litestd::{eprint, eprintln, print, println};
    reentrant!(litestd_io, NestedLite, print, println, eprint, eprintln);
}

fn child_reentrant_std() {
    reentrant!(std_io, NestedStd, print, println, eprint, eprintln);
}

/// Holds the stdout lock while another thread prints: its line must come
/// after everything written under the lock.
fn child_exclusive() {
    use litestd::io::Write;
    let mut lock = litestd::io::stdout().lock();
    std::thread::scope(|s| {
        s.spawn(|| litestd::println!("thread"));
        s.spawn(|| {
            litestd::io::stdout().write_all(b"write\n").unwrap();
        });
        std::thread::sleep(core::time::Duration::from_millis(100));
        lock.write_all(b"main 1\n").unwrap();
        litestd::println!("main 2");
        drop(lock);
    });
}

/// The kinds of record in the `threads` mode, one per way of writing.
const KINDS: usize = 6;
/// The length of the `print!` records that span several buffer writes.
const LONG: usize = 3000;

/// Eight threads write records in every way; each record must stay whole
/// and each thread's records in order.
fn child_threads() {
    use litestd::io::Write;
    std::thread::scope(|s| {
        for t in 0..8u8 {
            s.spawn(move || {
                let fill = char::from(b'a' + t);
                for i in 0..240 {
                    match i % KINDS {
                        0 => litestd::print!("{t}:{i}:p\n"),
                        1 => litestd::println!("{t}:{i}:l"),
                        2 => {
                            let record = format!("{t}:{i}:w\n");
                            litestd::io::stdout()
                                .write_all(record.as_bytes())
                                .unwrap();
                        }
                        3 => writeln!(litestd::io::stdout(), "{t}:{i}:f")
                            .unwrap(),
                        4 => {
                            let mut lock = litestd::io::stdout().lock();
                            write!(lock, "{t}:{i}:k").unwrap();
                            for _ in 0..5 {
                                lock.write_all(b"|part").unwrap();
                            }
                            lock.write_all(b"\n").unwrap();
                        }
                        _ => {
                            let long: String =
                                core::iter::repeat_n(fill, LONG).collect();
                            litestd::println!("{t}:{i}:{long}");
                        }
                    }
                }
            });
        }
    });
}

/// A `Display` implementation that fails after writing a little.
struct Failing;

impl fmt::Display for Failing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("half")?;
        Err(fmt::Error)
    }
}

/// `write!` reports a failing formatting trait, where std panics, after
/// writing what was formatted before the failure.
fn child_write_fmt_error() {
    use litestd::io::Write;
    let result = writeln!(litestd::io::stdout(), "a {Failing} b");
    litestd::println!();
    let error = result.unwrap_err();
    litestd::println!("{:?} {error}", error.kind());
}

/// `print!` panics on a failing formatting trait, as std's does, after
/// writing what was formatted before the failure.
fn child_print_fmt_error() {
    litestd::print!("c {} d", Failing);
}

/// Prints whether each stream is a terminal, to litestd and to std.
fn child_is_terminal() {
    use litestd::io::IsTerminal;
    let lite = [
        litestd::io::stdin().is_terminal(),
        litestd::io::stdout().is_terminal(),
        litestd::io::stderr().is_terminal(),
        litestd::io::stdin().lock().is_terminal(),
        litestd::io::stdout().lock().is_terminal(),
        litestd::io::stderr().lock().is_terminal(),
    ];
    let std = {
        use std::io::IsTerminal;
        [
            std::io::stdin().is_terminal(),
            std::io::stdout().is_terminal(),
            std::io::stderr().is_terminal(),
            std::io::stdin().lock().is_terminal(),
            std::io::stdout().lock().is_terminal(),
            std::io::stderr().lock().is_terminal(),
        ]
    };
    litestd::println!("lite {lite:?}");
    litestd::println!("std {std:?}");
}

/// With every stream on the null device: reads end at once and writes
/// succeed. Returns a bit per failed check.
fn child_null() -> i32 {
    use litestd::io::{BufRead, Read, Write};
    let mut failed = 0;
    // One statement per check: a `StdinLock` temporary lives to the end of
    // its statement, and a second lock in the same one would wait forever.
    let mut record = |i: u32, ok: bool| {
        if !ok {
            failed |= 1 << i;
        }
    };
    let mut line = String::new();
    record(0, litestd::io::stdin().read_line(&mut line).ok() == Some(0));
    record(1, litestd::io::stdin().lines().next().is_none());
    let mut all = Vec::new();
    record(
        2,
        litestd::io::stdin().read_to_end(&mut all).ok() == Some(0),
    );
    record(
        3,
        litestd::io::stdin()
            .lock()
            .fill_buf()
            .is_ok_and(<[u8]>::is_empty),
    );
    record(4, litestd::io::stdout().write(b"abc").ok() == Some(3));
    record(5, writeln!(litestd::io::stdout(), "{}", 1).is_ok());
    record(6, litestd::io::stderr().write_all(b"abc").is_ok());
    record(7, litestd::io::stdout().flush().is_ok());
    litestd::println!("still alive");
    failed
}

/// With every stream closed, or on Windows without a handle or with an
/// invalid one: reads end at once and writes succeed, as in std. Returns a
/// bit per failed check; std's checks come after litestd's.
fn child_closed(invalid: bool) -> i32 {
    close_std_streams(invalid);
    let mut failed = 0;
    let mut record = |i: u32, ok: bool| {
        if !ok {
            failed |= 1 << i;
        }
    };
    {
        use litestd::io::{BufRead, Read, Write};
        let mut line = String::new();
        let mut buf = [0u8; 4];
        record(0, litestd::io::stdin().read_line(&mut line).ok() == Some(0));
        record(1, litestd::io::stdin().read(&mut buf).ok() == Some(0));
        record(
            2,
            litestd::io::stdin()
                .lock()
                .fill_buf()
                .is_ok_and(<[u8]>::is_empty),
        );
        record(3, litestd::io::stdout().write(b"abc").ok() == Some(3));
        record(4, litestd::io::stdout().write_all(b"abc").is_ok());
        record(5, writeln!(litestd::io::stdout(), "{}", 1).is_ok());
        record(6, litestd::io::stdout().flush().is_ok());
        record(7, litestd::io::stderr().write(b"abc").ok() == Some(3));
        record(8, writeln!(litestd::io::stderr().lock(), "x").is_ok());
        litestd::println!("print {}", 1);
        litestd::eprintln!("eprint");
    }
    // std turns an invalid handle into the last error, which may be any.
    if !invalid {
        use std::io::{BufRead, Read, Write};
        let mut line = String::new();
        let mut buf = [0u8; 4];
        record(16, std::io::stdin().read_line(&mut line).ok() == Some(0));
        record(17, std::io::stdin().read(&mut buf).ok() == Some(0));
        record(
            18,
            std::io::stdin()
                .lock()
                .fill_buf()
                .is_ok_and(<[u8]>::is_empty),
        );
        record(19, std::io::stdout().write(b"abc").ok() == Some(3));
        record(20, writeln!(std::io::stdout(), "{}", 1).is_ok());
        record(21, std::io::stdout().flush().is_ok());
        record(22, std::io::stderr().write(b"abc").ok() == Some(3));
    }
    failed
}

/// Closes this process's standard streams: on Unix the descriptors, which
/// std's runtime would have reopened had the parent closed them; on
/// Windows by setting null handles, or `INVALID_HANDLE_VALUE` if `invalid`.
fn close_std_streams(invalid: bool) {
    let _ = invalid;
    #[cfg(any(unix, target_os = "wasi"))]
    for fd in 0..3 {
        // SAFETY: nothing in this process uses the standard descriptors
        // after this, and nothing else owns them.
        unsafe { libc::close(fd) };
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::INVALID_HANDLE_VALUE,
            System::Console::{
                STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
                SetStdHandle,
            },
        };
        let handle = if invalid {
            INVALID_HANDLE_VALUE
        } else {
            core::ptr::null_mut()
        };
        for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // SAFETY: only the process's standard handle slots change; the
            // handles they held stay open.
            let ok = unsafe { SetStdHandle(id, handle) };
            assert_ne!(ok, 0);
        }
    }
}

// The parent side.

/// Keeps the children from writing core files, which could land in the
/// working tree. The children inherit this process's limit.
#[cfg(all(unix, not(miri)))]
fn forbid_core_files() {
    let limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a valid `rlimit` for the duration of the call.
    let r = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &raw const limit) };
    assert_eq!(r, 0);
}

/// A command that runs this test binary as a child in `mode`.
#[cfg(not(miri))]
fn child_command(mode: &str) -> std::process::Command {
    #[cfg(unix)]
    forbid_core_files();
    let mut command =
        std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "child", "--nocapture", "--test-threads=1", "-q"])
        .env("LITESTD_CHILD", mode);
    command
}

/// Starts `command` with its standard streams on pipes, stdin empty.
#[cfg(not(miri))]
fn piped(mut command: std::process::Command) -> std::process::Child {
    use std::process::Stdio;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Returns what `bytes` holds after `MARK`.
#[cfg(not(miri))]
fn after_mark(bytes: Vec<u8>) -> String {
    let text = String::from_utf8(bytes).unwrap();
    let (_, rest) = text.split_once(MARK).expect("child output marker");
    rest.to_owned()
}

/// Waits for `child` and collects its output, killing it if it runs for
/// more than a minute, so that a deadlock fails the test instead of
/// hanging it.
#[cfg(not(miri))]
fn wait_for(mut child: std::process::Child) -> std::process::Output {
    use std::io::Read;
    fn drain(
        pipe: Option<impl Read + Send + 'static>,
    ) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                pipe.read_to_end(&mut bytes).unwrap();
            }
            bytes
        })
    }
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let start = std::time::Instant::now();
    // Ends when the child does, or kills it after a minute.
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > core::time::Duration::from_secs(60) {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("the child did not finish within a minute");
        }
        std::thread::sleep(core::time::Duration::from_millis(5));
    };
    std::process::Output {
        status,
        stdout: stdout.join().unwrap(),
        stderr: stderr.join().unwrap(),
    }
}

/// Runs `mode` with `stdin` as its standard input, feeding it `input`
/// through a pipe if there is some, and returns what it printed after
/// `MARK` to stdout and stderr.
#[cfg(not(miri))]
fn run_with(
    mode: &str,
    stdin: std::process::Stdio,
    input: Option<Vec<u8>>,
) -> (String, String) {
    use std::process::Stdio;
    let mut child = child_command(mode)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            stdin
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let feeder = input.map(|input| {
        let pipe = child.stdin.take().unwrap();
        std::thread::spawn(move || feed(pipe, &input))
    });
    let out = wait_for(child);
    if let Some(feeder) = feeder {
        feeder.join().unwrap();
    }
    assert!(out.status.success(), "{mode}: {out:?}");
    (after_mark(out.stdout), after_mark(out.stderr))
}

/// Runs `mode` on `input` and returns what it printed.
#[cfg(not(miri))]
fn run(mode: &str, input: &[u8]) -> (String, String) {
    run_with(mode, std::process::Stdio::null(), Some(input.to_vec()))
}

/// Writes `input` to `pipe` in pieces of odd sizes, pausing now and then,
/// so that the child's reads return lines in several chunks. A child that
/// stops reading early ends the feeding.
#[cfg(not(miri))]
fn feed(mut pipe: std::process::ChildStdin, input: &[u8]) {
    use std::io::Write;
    let mut rest = input;
    for (i, size) in [1, 7, 3000, 1, 9000, 17].into_iter().cycle().enumerate() {
        if rest.is_empty() {
            break;
        }
        let (piece, tail) = rest.split_at(size.min(rest.len()));
        if pipe.write_all(piece).and_then(|()| pipe.flush()).is_err() {
            return;
        }
        rest = tail;
        if i % 4 == 0 {
            std::thread::sleep(core::time::Duration::from_millis(1));
        }
    }
}

/// Text with long, short and empty lines, both line endings, and a last
/// line without one, spanning several 8 KiB buffers.
#[cfg(not(miri))]
fn lines_input() -> Vec<u8> {
    let mut input = Vec::new();
    for i in 0..300 {
        input.extend_from_slice(
            format!("line {i} {}\n", "w".repeat(i % 70)).as_bytes(),
        );
        if i % 50 == 0 {
            input.extend_from_slice(b"\n\r\nwindows\r\n");
            input.extend_from_slice("unicode \u{e9}\u{4e16}\n".as_bytes());
            input.resize(input.len() + 20_000, b'y');
            input.push(b'\n');
        }
    }
    input.extend_from_slice(b"no newline at the end");
    input
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn read_line_matches_std() {
    let input = lines_input();
    let (lite, _) = run("read_line_lite", &input);
    let (std, _) = run("read_line_std", &input);
    assert_eq!(lite, std);
    let mut expected = String::new();
    for line in String::from_utf8(input).unwrap().split_inclusive('\n') {
        fmt::Write::write_fmt(
            &mut expected,
            format_args!("{} {line:?}\n", line.len()),
        )
        .unwrap();
    }
    assert_eq!(lite, expected);
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn lines_match_std() {
    let mut input = lines_input();
    // Invalid UTF-8 fails its line, which is consumed.
    input.extend_from_slice(b"\nbad \xff line\nafter\r\n");
    let (lite, _) = run("lines_lite", &input);
    let (std, _) = run("lines_std", &input);
    assert_eq!(lite, std);
    assert!(
        lite.ends_with(
            "\"no newline at the end\"\nerror invalid data\n\"after\"\n"
        ),
        "{lite}"
    );
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn read_apis_match_std() {
    let input = b"hello, world; skipped; line\nabcdefgh,ijk rest of it";
    let (lite, _) = run("read_apis_lite", input);
    let (std, _) = run("read_apis_std", input);
    assert_eq!(lite, std);
    assert!(
        lite.starts_with("exact [104, 101, 108, 108, 111]\nfill 44\n"),
        "{lite}"
    );
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn read_to_end_reads_everything() {
    let input: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let (out, _) = run("read_to_end", &input);
    assert_eq!(out, format!("{} {}\n", input.len(), checksum(&input)));
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn writes_keep_their_order_as_in_std() {
    let lite = run_with("order_lite", std::process::Stdio::null(), None);
    let std = run_with("order_std", std::process::Stdio::null(), None);
    assert_eq!(lite, std);
    assert!(lite.0.starts_with("a0bc\nd0\ne\nfg\na1bc\n"), "{}", lite.0);
    assert!(lite.1.starts_with("x0yz\nx1yz\n"), "{}", lite.1);
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn locks_are_reentrant_as_in_std() {
    let lite = run_with("reentrant_lite", std::process::Stdio::null(), None);
    let std = run_with("reentrant_std", std::process::Stdio::null(), None);
    assert_eq!(lite, std);
    assert_eq!(lite.0, "123\n<inner[w]>\n4\ninner[w]x<inner[w]>y\n");
    assert_eq!(lite.1, "[e]e1 e2\ne3 <[e]>\n[e]done\n");
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn a_lock_excludes_other_threads() {
    let (out, _) = run_with("exclusive", std::process::Stdio::null(), None);
    let (main, rest) = out.split_at("main 1\nmain 2\n".len());
    assert_eq!(main, "main 1\nmain 2\n");
    let mut rest: Vec<&str> = rest.lines().collect();
    rest.sort_unstable();
    assert_eq!(rest, ["thread", "write"]);
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn records_from_threads_stay_whole_and_in_order() {
    let (out, _) = run_with("threads", std::process::Stdio::null(), None);
    let mut next = [0usize; 8];
    for line in out.lines() {
        let mut parts = line.splitn(3, ':');
        let t: usize = parts.next().unwrap().parse().unwrap();
        let i: usize = parts.next().unwrap().parse().unwrap();
        let body = parts.next().unwrap();
        assert_eq!(i, next[t], "thread {t} out of order: {line:.80}");
        next[t] += 1;
        let fill = char::from(b'a' + u8::try_from(t).unwrap());
        let expected = match i % KINDS {
            0 => "p".to_owned(),
            1 => "l".to_owned(),
            2 => "w".to_owned(),
            3 => "f".to_owned(),
            4 => format!("k{}", "|part".repeat(5)),
            _ => core::iter::repeat_n(fill, LONG).collect(),
        };
        assert!(body == expected, "torn record: {line:.80}");
    }
    assert_eq!(next, [240; 8]);
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn write_fmt_reports_formatter_errors() {
    let (out, _) =
        run_with("write_fmt_error", std::process::Stdio::null(), None);
    assert_eq!(out, "a half\nUncategorized formatter error\n");
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn print_panics_on_formatter_errors() {
    let out = wait_for(piped(child_command("print_fmt_error")));
    assert!(!out.status.success(), "{out:?}");
    let stdout = after_mark(out.stdout);
    assert!(stdout.starts_with("c half"), "{stdout:?}");
    assert!(!stdout.starts_with("c half d"), "{stdout:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
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
fn pipes_are_not_terminals() {
    let (out, _) = run("is_terminal", b"");
    let expected = "lite [false, false, false, false, false, false]\n\
                    std [false, false, false, false, false, false]\n";
    assert_eq!(out, expected);
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn null_streams_read_nothing_and_take_everything() {
    use std::process::Stdio;
    let child = child_command("null")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let status = wait_for(child).status;
    assert_eq!(status.code(), Some(0), "failed checks: {status:?}");
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn closed_streams_read_nothing_and_take_everything() {
    let out = wait_for(piped(child_command("closed")));
    assert_eq!(out.status.code(), Some(0), "failed checks: {out:?}");
}

#[cfg(all(windows, not(miri)))]
#[test]
fn invalid_handles_read_nothing_and_take_everything() {
    let out = wait_for(piped(child_command("invalid")));
    assert_eq!(out.status.code(), Some(0), "failed checks: {out:?}");
}

/// A parent may pass a standard input opened for overlapped I/O, on which
/// a read can still be in flight when the call that started it returns.
/// Here the parent writes slowly, so that the child's reads wait for data,
/// and each must return only once the kernel is done with its buffer.
#[cfg(all(windows, not(miri)))]
#[test]
fn overlapped_stdin_reads_every_line() {
    use std::{fs::OpenOptions, io::Write, os::windows::fs::OpenOptionsExt};

    let name =
        format!(r"\\.\pipe\litestd-overlapped-in-{}", std::process::id());
    let mut server = windows_pipe::create(&name, windows_pipe::OUTBOUND, 64);
    let client = OpenOptions::new()
        .read(true)
        .custom_flags(windows_pipe::FILE_FLAG_OVERLAPPED)
        .open(&name)
        .unwrap();
    let child = child_command("read_line_lite")
        .stdin(client)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut expected = String::new();
    for i in 0..40 {
        let line = format!("line {i} {}\n", "z".repeat(i * 7));
        server.write_all(line.as_bytes()).unwrap();
        fmt::Write::write_fmt(
            &mut expected,
            format_args!("{} {line:?}\n", line.len()),
        )
        .unwrap();
        if i % 8 == 0 {
            std::thread::sleep(core::time::Duration::from_millis(20));
        }
    }
    drop(server);
    let out = wait_for(child);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(after_mark(out.stdout), expected);
}

/// A child whose stdout is an msys pseudo-terminal's pipe sees a terminal,
/// with litestd as with std.
#[cfg(all(windows, not(miri)))]
#[test]
fn msys_stdout_is_a_terminal() {
    use std::{fs::OpenOptions, io::Read};

    let name = format!(r"\\.\pipe\msys-{}-pty9-to-master", std::process::id());
    let mut server = windows_pipe::create(&name, windows_pipe::INBOUND, 4096);
    let client = OpenOptions::new().write(true).open(&name).unwrap();
    let child = child_command("is_terminal")
        .stdout(client)
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut out = Vec::new();
    server.read_to_end(&mut out).unwrap();
    let status = wait_for(child).status;
    assert!(status.success(), "{status:?}");
    let out = after_mark(out);
    let expected = "lite [false, true, false, false, true, false]\n\
                    std [false, true, false, false, true, false]\n";
    assert_eq!(out, expected);
}

/// Bytes that a pipe must get unchanged: some that are not UTF-8, and
/// characters split between writes.
#[cfg(windows)]
const RAW_PIECES: [&[u8]; 5] = [
    b"\xff\xfe",
    b"a\xe2\x82",
    b"\xac\xe4",
    b"x\xf0\x9f",
    b"\x98\x80\n",
];

/// Writes `RAW_PIECES` to stdout, one `write_all` each.
#[cfg(windows)]
fn child_raw_bytes() {
    use litestd::io::Write;
    let mut out = litestd::io::stdout();
    for piece in RAW_PIECES {
        out.write_all(piece).unwrap();
    }
}

/// Points stdout and stderr at a console screen buffer of its own, whose
/// code page shows UTF-8 bytes as other characters, and writes to it
/// through litestd. Prints what the writes returned, then the screen.
#[cfg(windows)]
fn child_console() {
    use windows_sys::Win32::{
        Foundation::{
            GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
        },
        Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE},
        System::Console::{
            AllocConsole, CONSOLE_TEXTMODE_BUFFER, CreateConsoleScreenBuffer,
            GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
            SetConsoleOutputCP, SetStdHandle,
        },
    };
    // A child started without a console window has a console already, and
    // then `AllocConsole` fails harmlessly.
    // SAFETY: `AllocConsole` has no preconditions.
    unsafe { AllocConsole() };
    // SAFETY: both pointers may be null; the rest are plain values.
    let screen = unsafe {
        CreateConsoleScreenBuffer(
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            core::ptr::null(),
            CONSOLE_TEXTMODE_BUFFER,
            core::ptr::null(),
        )
    };
    let error = std::io::Error::last_os_error();
    assert_ne!(screen, INVALID_HANDLE_VALUE, "no console: {error}");
    // SAFETY: `SetConsoleOutputCP` has no preconditions.
    assert_ne!(unsafe { SetConsoleOutputCP(437) }, 0);
    let ids = [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE];
    // SAFETY: `GetStdHandle` has no preconditions.
    let pipes = ids.map(|id| unsafe { GetStdHandle(id) });
    let point = |handles: [HANDLE; 2]| {
        for (id, handle) in ids.into_iter().zip(handles) {
            // SAFETY: only the process's standard handle slots change; the
            // handles they held stay open.
            assert_ne!(unsafe { SetStdHandle(id, handle) }, 0);
        }
    };
    point([screen; 2]);
    let report = console_writes();
    let text = read_screen(screen);
    // With the UTF-8 code page the console takes the bytes unchanged, as in
    // std: those that are not UTF-8, and a split character at once.
    // SAFETY: `SetConsoleOutputCP` has no preconditions.
    assert_ne!(unsafe { SetConsoleOutputCP(65001) }, 0);
    let mut out = litestd::io::stdout();
    let utf8 = [&b"\xff"[..], b"\xe2\x82", b"\xac"]
        .map(|bytes| litestd::io::Write::write(&mut out, bytes));
    point(pipes);
    litestd::println!("{report}");
    litestd::println!("{text}");
    litestd::println!("{utf8:?}");
}

/// Writes to the console that stdout and stderr point at, in whole and in
/// split characters, and returns what the `write` calls returned: for each
/// piece, the counts until it is written, or the error that ended it.
#[cfg(windows)]
fn console_writes() -> String {
    use litestd::io::{IsTerminal, Write};
    let long = format!("c{}", "\u{1d400}".repeat(300));
    // Whether to write to stderr, and the bytes.
    let pieces: [(bool, &[u8]); 12] = [
        // U+1D11E over three pieces; a carried character takes a byte per
        // write.
        (false, b"\xf0"),
        (false, b"\x9d\x84"),
        (false, b"\x9e!"),
        // stderr keeps the start of U+20AC apart from stdout's of U+E9.
        (false, b"\xc3"),
        (true, b"\xe2\x82"),
        (true, b"\xac"),
        (false, b"\xa9"),
        // Bytes that are not UTF-8 fail, but those before them go out; a
        // byte that cannot continue the carried ones fails and drops them.
        (false, b"ok\xffno"),
        (false, b"\xe4"),
        (false, b"x"),
        (false, b"x"),
        // Longer than one console write, which ends inside a character.
        (false, long.as_bytes()),
    ];
    let mut out = litestd::io::stdout();
    let mut err = litestd::io::stderr();
    let mut report = format!("terminal {}", out.is_terminal());
    litestd::print!("<a\u{e9}\u{416}\u{20ac}\u{1d400}>");
    for (to_stderr, piece) in pieces {
        report.push_str(" |");
        let mut rest = piece;
        // Each pass takes a byte at least, or ends the piece.
        while !rest.is_empty() {
            let result = if to_stderr {
                err.write(rest)
            } else {
                out.write(rest)
            };
            match result {
                Ok(n) if n > 0 => {
                    fmt::Write::write_fmt(&mut report, format_args!(" {n}"))
                        .unwrap();
                    rest = &rest[n..];
                }
                result => {
                    let result = result.map_err(|e| e.kind());
                    fmt::Write::write_fmt(
                        &mut report,
                        format_args!(" {result:?}"),
                    )
                    .unwrap();
                    break;
                }
            }
        }
    }
    // Through the print buffer, which writes it all.
    litestd::print!("{}", format!("b{}", "\u{e9}".repeat(600)));
    litestd::eprint!(">");
    report
}

/// Returns the text on the console screen buffer `screen`, without the
/// blank cells after it.
#[cfg(windows)]
fn read_screen(screen: windows_sys::Win32::Foundation::HANDLE) -> String {
    use windows_sys::Win32::System::Console::{
        COORD, ReadConsoleOutputCharacterW,
    };
    let mut cells = vec![0u16; 4096];
    let mut read = 0;
    // SAFETY: `cells` is writable for its length, and `read` for a `u32`.
    let ok = unsafe {
        ReadConsoleOutputCharacterW(
            screen,
            cells.as_mut_ptr(),
            u32::try_from(cells.len()).unwrap(),
            COORD { X: 0, Y: 0 },
            &raw mut read,
        )
    };
    assert_ne!(ok, 0, "{}", std::io::Error::last_os_error());
    cells.truncate(read as usize);
    String::from_utf16_lossy(&cells).trim_end().to_owned()
}

/// A pipe gets the bytes unchanged, UTF-8 or not, as in std.
#[cfg(all(windows, not(miri)))]
#[test]
fn pipes_get_bytes_unchanged() {
    let out = wait_for(piped(child_command("raw_bytes")));
    assert!(out.status.success(), "{out:?}");
    let mark = out
        .stdout
        .windows(MARK.len())
        .position(|window| window == MARK.as_bytes())
        .expect("child output marker");
    assert_eq!(out.stdout[mark + MARK.len()..], RAW_PIECES.concat());
}

/// A console gets text as UTF-16, as in std, so it shows every character
/// whatever its code page: a child writes to a console of its own and
/// reads the screen back.
#[cfg(all(windows, not(miri)))]
#[test]
fn consoles_show_text_whatever_their_code_page() {
    use std::os::windows::process::CommandExt;

    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
    let mut command = child_command("console");
    command.creation_flags(CREATE_NO_WINDOW);
    let out = wait_for(piped(command));
    assert!(out.status.success(), "{out:?}");
    let report = "terminal true | 1 | 1 1 | 1 1 | 1 | 1 1 | 1 | 1 \
                  | 2 Err(InvalidData) | 1 | Err(InvalidData) | 1 | 1021 180";
    let screen = format!(
        "<a\u{e9}\u{416}\u{20ac}\u{1d400}>\u{1d11e}!\u{20ac}\u{e9}okxc{}b{}>",
        "\u{1d400}".repeat(300),
        "\u{e9}".repeat(600),
    );
    let utf8 = "[Ok(1), Ok(2), Ok(1)]";
    let out = after_mark(out.stdout);
    // Windows' console hands each character outside the BMP back from its
    // cells as U+FFFD; wine gives back the surrogates written.
    let screen: String = if out.contains('\u{fffd}') {
        let bmp = |c| if c > '\u{ffff}' { '\u{fffd}' } else { c };
        screen.chars().map(bmp).collect()
    } else {
        screen
    };
    assert_eq!(out, format!("{report}\n{screen}\n{utf8}\n"));
}
