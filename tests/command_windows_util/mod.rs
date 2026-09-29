//! Helpers for the Windows `process` tests: this test binary run as a child
//! that reports what it received, the program and command line of a
//! suspended child read from the kernel, and temporary directories.
//!
//! A child runs only the `child_main` test, which finds `@litestd-child`
//! and its mode after the harness's arguments, and prints `@@` on a line of
//! its own before its report, after the test harness's own output. The
//! environment stays unchanged, so that tests can compare inheriting it.

#![allow(
    dead_code,
    clippy::panic,
    clippy::redundant_pub_crate,
    clippy::unwrap_used,
    reason = "test helpers, used by some tests each, fail the test on errors"
)]

use core::{
    ffi::c_void,
    fmt::Write as _,
    sync::atomic::{AtomicUsize, Ordering},
};
use std::{
    ffi::{OsStr, OsString},
    io::{Read as _, Write as _},
    os::windows::{
        ffi::{OsStrExt as _, OsStringExt as _},
        io::AsRawHandle as _,
        process::CommandExt as _,
    },
    path::{Path, PathBuf},
};

use litestd::os::windows::{
    ffi::OsStringExt as _, io::AsRawHandle as _, process::CommandExt as _,
};
use windows_sys::Win32::Foundation::GetHandleInformation;

/// Marks a child among the free arguments; the mode follows it.
const MARKER: &str = "@litestd-child";

/// The arguments that make the test harness of a child run only
/// `child_main`, and mark the child; the mode and the arguments of a test
/// follow. Free arguments are name filters to the harness, which none of
/// the tests' names matches.
pub(crate) const HARNESS: [&str; 7] = [
    "--exact",
    "child_main",
    "--nocapture",
    "--test-threads=1",
    "-q",
    "--",
    MARKER,
];

const CREATE_SUSPENDED: u32 = 0x4;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationProcess(
        process: *mut c_void,
        class: u32,
        info: *mut c_void,
        len: u32,
        returned: *mut u32,
    ) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn ReadProcessMemory(
        process: *mut c_void,
        address: *const c_void,
        buf: *mut c_void,
        len: usize,
        read: *mut usize,
    ) -> i32;
    fn QueryFullProcessImageNameW(
        process: *mut c_void,
        flags: u32,
        name: *mut u16,
        size: *mut u32,
    ) -> i32;
    fn GetCommandLineW() -> *const u16;
    fn GetEnvironmentStringsW() -> *mut u16;
    fn FreeEnvironmentStringsW(block: *mut u16) -> i32;
    fn GetCurrentDirectoryW(len: u32, buf: *mut u16) -> u32;
}

/// A directory of its own for one test, which std removes when dropped.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    /// Creates `<temp>/litestd-cmd-<name>-<pid>-<n>`.
    pub(crate) fn new(name: &str) -> Self {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        let dir = format!("litestd-cmd-{name}-{}-{n}", std::process::id());
        let path = std::env::temp_dir().join(dir);
        if path.exists() {
            std::fs::remove_dir_all(&path).unwrap();
        }
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    pub(crate) fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Children may still hold files for a moment after they exit.
        for _ in 0..50 {
            if std::fs::remove_dir_all(&self.0).is_ok() || !self.0.exists() {
                return;
            }
            std::thread::sleep(core::time::Duration::from_millis(20));
        }
    }
}

/// The same string as a litestd `OsString`.
pub(crate) fn lite(s: impl AsRef<OsStr>) -> litestd::ffi::OsString {
    let wide: Vec<u16> = s.as_ref().encode_wide().collect();
    litestd::ffi::OsString::from_wide(&wide)
}

/// The same string as a std `OsString`.
pub(crate) fn real(s: &litestd::ffi::OsStr) -> OsString {
    use litestd::os::windows::ffi::OsStrExt as _;
    let wide: Vec<u16> = s.encode_wide().collect();
    OsString::from_wide(&wide)
}

/// Whether the tests run under wine, whose `ntdll` exports
/// `wine_get_version`.
pub(crate) fn under_wine() -> bool {
    use windows_sys::{
        Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress},
        core::w,
    };
    // SAFETY: the name is NUL-terminated UTF-16, and the handle, which is
    // not counted, stays valid: every process keeps `ntdll` loaded.
    let ntdll = unsafe { GetModuleHandleW(w!("ntdll.dll")) };
    if ntdll.is_null() {
        return false;
    }
    // SAFETY: `ntdll` is a loaded module, and the name a C string.
    let export =
        unsafe { GetProcAddress(ntdll, c"wine_get_version".as_ptr().cast()) };
    export.is_some()
}

/// This test binary.
pub(crate) fn exe() -> PathBuf {
    std::env::current_exe().unwrap()
}

/// A litestd command that runs this binary as a child in `mode`.
pub(crate) fn lite_child(mode: &str) -> litestd::process::Command {
    let mut cmd = litestd::process::Command::new(lite(exe()));
    cmd.args(HARNESS).arg(mode);
    cmd
}

/// A std command that runs this binary as a child in `mode`.
pub(crate) fn std_child(mode: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new(exe());
    cmd.args(HARNESS).arg(mode);
    cmd
}

/// The report of a child: what follows its `@@` line.
pub(crate) fn payload(stdout: &[u8]) -> &[u8] {
    let marker = b"@@\n";
    let Some(at) = stdout.windows(marker.len()).position(|w| w == marker)
    else {
        panic!("no report in {:?}", String::from_utf8_lossy(stdout));
    };
    &stdout[at + marker.len()..]
}

/// The UTF-16 units of `s` in hex, four digits each.
pub(crate) fn hex(units: impl IntoIterator<Item = u16>) -> String {
    units.into_iter().fold(String::new(), |mut s, u| {
        write!(s, "{u:04x}").unwrap();
        s
    })
}

/// The lines of a child's report.
pub(crate) fn report_lines(stdout: &[u8]) -> Vec<String> {
    let text = core::str::from_utf8(payload(stdout)).unwrap();
    text.lines().map(str::to_owned).collect()
}

/// The child side of the tests: reports what its mode asks for, then exits
/// before the test harness prints more. A no-op in the parent.
pub(crate) fn child_main() {
    let mut args = std::env::args_os().skip_while(|a| a != "--").skip(1);
    if args.next().is_none_or(|a| a != MARKER) {
        return;
    }
    let mode = args.next().and_then(|m| m.into_string().ok());
    let mode = mode.unwrap_or_default();
    let (name, param) = mode.split_once(':').unwrap_or((&mode, ""));
    let mut out = std::io::stdout().lock();
    let code = report(name, param, args, &mut out).and_then(|code| {
        out.flush()?;
        Ok(code)
    });
    std::process::exit(code.unwrap_or(-2))
}

/// An error for a mode or parameter that the child does not understand.
fn bad(e: impl core::fmt::Display) -> std::io::Error {
    std::io::Error::other(format!("{e}"))
}

/// Writes the report of mode `name` with `param` and the arguments after
/// it to `out`, and returns the exit code.
fn report(
    name: &str,
    param: &str,
    args: impl Iterator<Item = OsString>,
    out: &mut impl std::io::Write,
) -> std::io::Result<i32> {
    out.write_all(b"@@\n")?;
    match name {
        // Each argument after the harness's, in hex, a line each.
        "args" => {
            for arg in args {
                writeln!(out, "{}", hex(arg.encode_wide()))?;
            }
        }
        // The raw command line, in hex.
        "cmdline" => {
            // SAFETY: the command line is NUL-terminated and lives as long
            // as the process.
            let line = unsafe { wide_c_str(GetCommandLineW()) };
            writeln!(out, "{}", hex(line.iter().copied()))?;
        }
        // Each string of the environment block, in hex, a line each.
        "env" => {
            for var in environment_block() {
                writeln!(out, "{}", hex(var))?;
            }
        }
        // The working directory, in hex.
        "cwd" => {
            let mut buf = [0u16; 1024];
            // SAFETY: `buf` is valid for writes of its length.
            let len = unsafe { GetCurrentDirectoryW(1024, buf.as_mut_ptr()) };
            let dir = buf.iter().copied().take(len as usize);
            writeln!(out, "{}", hex(dir))?;
        }
        "exit" => {
            let code = param.parse::<u32>().map_err(bad)?;
            return Ok(i32::from_ne_bytes(code.to_ne_bytes()));
        }
        // Copies stdin to stdout.
        "echo" => {
            std::io::copy(&mut std::io::stdin().lock(), out)?;
        }
        // Writes `param` bytes to each of stdout and stderr, alternating
        // in chunks, so that a parent reading only one of them deadlocks.
        "flood" => {
            let mut left: u64 = param.parse().map_err(bad)?;
            let mut err = std::io::stderr().lock();
            while left > 0 {
                let n = left.min(4096);
                std::io::copy(&mut std::io::repeat(b'o').take(n), out)?;
                std::io::copy(&mut std::io::repeat(b'e').take(n), &mut err)?;
                left -= n;
            }
        }
        "stderr" => {
            std::io::stderr().write_all(b"to stderr")?;
            out.write_all(b"to stdout")?;
        }
        "sleep" => std::thread::sleep(core::time::Duration::from_secs(60)),
        // Resolves the program `param` with the environment unchanged, so
        // that this process's `PATH` is searched, and prints the program
        // that both litestd and std start.
        "resolve" => {
            let a = lite_snapshot(&mut litestd::process::Command::new(param));
            let b = std_snapshot(&mut std::process::Command::new(param));
            if let Some((a, b)) = same(param, a, b) {
                assert_eq!(a, b);
                writeln!(out, "{}", a.image)?;
            }
        }
        // Spawns children with pipes, and prints how many more handles this
        // process has afterwards: none, if each was closed.
        "leaks" => writeln!(out, "{}", handle_growth())?,
        // Starts a child in the `stderr` mode, through litestd or std as
        // `param` says, with its stdout sent to this process's stdout or
        // stderr, and exits with its code.
        #[cfg(feature = "stdio")]
        "redirect" => {
            out.flush()?;
            let (library, stream) = param.split_once(':').unwrap_or_default();
            let status = match (library, stream) {
                ("lite", "out") => lite_redirect(litestd::io::stdout()),
                ("lite", _) => lite_redirect(litestd::io::stderr()),
                ("std", "out") => std_redirect(std::io::stdout()),
                (_, _) => std_redirect(std::io::stderr()),
            };
            return Ok(status);
        }
        other => return Err(bad(format!("unknown child mode {other}"))),
    }
    Ok(0)
}

/// Runs a child in the `stderr` mode with litestd, its stdout on `stream`.
#[cfg(feature = "stdio")]
fn lite_redirect(stream: impl Into<litestd::process::Stdio>) -> i32 {
    let status = lite_child("stderr").stdout(stream).status();
    status.map_or(-3, |s| s.code().unwrap_or(-4))
}

/// Runs a child in the `stderr` mode with std, its stdout on `stream`.
#[cfg(feature = "stdio")]
fn std_redirect(stream: impl Into<std::process::Stdio>) -> i32 {
    let status = std_child("stderr").stdout(stream).status();
    status.map_or(-3, |s| s.code().unwrap_or(-4))
}

/// The handles this process gains over spawning and reaping children in
/// every way that makes handles, after a first round that opens what
/// litestd keeps open for good.
fn handle_growth() -> i64 {
    use litestd::process::Stdio;
    // Counts the open handles by probing each value, as wine's
    // `GetProcessHandleCount` always reports none.
    let count = || {
        let open = (1..0x4000_usize).filter(|&i| {
            let mut flags = 0;
            let handle = core::ptr::without_provenance_mut(i * 4);
            // SAFETY: querying any handle value is harmless, and `flags`
            // is writable.
            unsafe { GetHandleInformation(handle, &raw mut flags) != 0 }
        });
        i64::try_from(open.count()).unwrap()
    };
    let round = || {
        let out = lite_child("stderr").output().unwrap();
        assert!(out.status.success());
        let mut child = lite_child("echo")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let second = lite_child("echo")
            .stdin(Stdio::from(child.stdout.take().unwrap()))
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        drop(child.stdin.take());
        second.wait_with_output().unwrap();
        child.wait().unwrap();
        let mut killed =
            lite_child("sleep").stdout(Stdio::null()).spawn().unwrap();
        killed.kill().unwrap();
        killed.wait().unwrap();
        assert!(
            litestd::process::Command::new("litestd-missing")
                .spawn()
                .is_err()
        );
    };
    round();
    // Relay threads end, and close their pipes, soon after their children.
    std::thread::sleep(core::time::Duration::from_millis(200));
    let before = count();
    for _ in 0..5 {
        round();
    }
    std::thread::sleep(core::time::Duration::from_millis(200));
    count() - before
}

/// The NUL-terminated string at `s`.
///
/// # Safety
///
/// `s` must point to a NUL-terminated string that outlives the result.
const unsafe fn wide_c_str<'a>(s: *const u16) -> &'a [u16] {
    let mut len = 0;
    // SAFETY: the string is NUL-terminated, per the caller.
    while unsafe { *s.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: the first `len` units are the string, per the caller.
    unsafe { core::slice::from_raw_parts(s, len) }
}

/// The strings of this process's environment block, in order.
fn environment_block() -> Vec<Vec<u16>> {
    // SAFETY: no preconditions.
    let block = unsafe { GetEnvironmentStringsW() };
    assert!(!block.is_null());
    let mut vars = Vec::new();
    let mut at = block.cast_const();
    loop {
        // SAFETY: the block holds NUL-terminated strings, then an empty one.
        let var = unsafe { wide_c_str(at) };
        if var.is_empty() {
            break;
        }
        vars.push(var.to_vec());
        // SAFETY: the next string starts after this one's NUL.
        at = unsafe { at.add(var.len() + 1) };
    }
    // SAFETY: the block came from `GetEnvironmentStringsW`.
    unsafe { FreeEnvironmentStringsW(block) };
    vars
}

/// The program and command line that a suspended child was started with.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub(crate) image: String,
    pub(crate) command_line: Vec<u16>,
}

/// Where the process parameters are in the PEB, and the command line in
/// them.
#[cfg(target_pointer_width = "64")]
const OFFSETS: (usize, usize) = (0x20, 0x70);
#[cfg(target_pointer_width = "32")]
const OFFSETS: (usize, usize) = (0x10, 0x40);

/// Reads `len` bytes at `address` in `process`.
fn read_memory(process: *mut c_void, address: usize, buf: &mut [u8]) {
    let mut read = 0;
    // SAFETY: `buf` is valid for writes of its length, and `read` too.
    let ok = unsafe {
        ReadProcessMemory(
            process,
            address as *const c_void,
            buf.as_mut_ptr().cast(),
            buf.len(),
            &raw mut read,
        )
    };
    assert!(
        ok != 0 && read == buf.len(),
        "{}",
        std::io::Error::last_os_error()
    );
}

/// The program and command line of the suspended process `process`.
fn snapshot(process: *mut c_void) -> Snapshot {
    const WORD: usize = size_of::<usize>();
    let mut basic = [0usize; 6];
    let mut returned = 0;
    // SAFETY: `basic` has the size of `PROCESS_BASIC_INFORMATION`, class 0.
    let status = unsafe {
        NtQueryInformationProcess(
            process,
            0,
            basic.as_mut_ptr().cast(),
            u32::try_from(size_of_val(&basic)).unwrap(),
            &raw mut returned,
        )
    };
    assert!(status >= 0, "{status:#x}");
    let peb = basic[1];
    let mut word = [0u8; WORD];
    read_memory(process, peb + OFFSETS.0, &mut word);
    let params = usize::from_ne_bytes(word);
    // The command line's `UNICODE_STRING`: a length, then a pointer.
    let mut string = [0u8; 2 * WORD];
    read_memory(process, params + OFFSETS.1, &mut string);
    let len = usize::from(u16::from_ne_bytes([string[0], string[1]]));
    let buffer = usize::from_ne_bytes(string[WORD..].try_into().unwrap());
    let mut bytes = vec![0u8; len];
    read_memory(process, buffer, &mut bytes);
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|b| u16::from_ne_bytes([b[0], b[1]]))
        .collect();
    let mut name = [0u16; 1024];
    let mut size = 1024;
    // SAFETY: `name` is valid for writes of `size` units.
    let ok = unsafe {
        QueryFullProcessImageNameW(process, 0, name.as_mut_ptr(), &raw mut size)
    };
    assert!(ok != 0, "{}", std::io::Error::last_os_error());
    Snapshot {
        image: String::from_utf16_lossy(&name[..size as usize]),
        command_line: units,
    }
}

/// Starts `cmd` with litestd, suspended, and returns its snapshot.
pub(crate) fn lite_snapshot(
    cmd: &mut litestd::process::Command,
) -> litestd::io::Result<Snapshot> {
    cmd.creation_flags(CREATE_SUSPENDED);
    let mut child = cmd.spawn()?;
    let snapshot = snapshot(child.as_raw_handle());
    child.kill().unwrap();
    child.wait().unwrap();
    Ok(snapshot)
}

/// Starts `cmd` with std, suspended, and returns its snapshot.
pub(crate) fn std_snapshot(
    cmd: &mut std::process::Command,
) -> std::io::Result<Snapshot> {
    cmd.creation_flags(CREATE_SUSPENDED);
    let mut child = cmd.spawn()?;
    let snapshot = snapshot(child.as_raw_handle());
    child.kill().unwrap();
    child.wait().unwrap();
    Ok(snapshot)
}

/// Asserts that litestd and std failed alike: same kind, OS code and
/// message.
pub(crate) fn same_error(
    what: &str,
    lite: &litestd::io::Error,
    real: &std::io::Error,
) {
    assert_eq!(
        format!("{:?}", lite.kind()),
        format!("{:?}", real.kind()),
        "{what}: error kind ({lite} vs {real})"
    );
    assert_eq!(lite.raw_os_error(), real.raw_os_error(), "{what}: OS error");
    assert_eq!(lite.to_string(), real.to_string(), "{what}: message");
}

/// Asserts that two results agree: both succeed, or both fail alike.
pub(crate) fn same<T, U>(
    what: &str,
    lite: litestd::io::Result<T>,
    real: std::io::Result<U>,
) -> Option<(T, U)> {
    match (lite, real) {
        (Ok(a), Ok(b)) => Some((a, b)),
        (Err(a), Err(b)) => {
            same_error(what, &a, &b);
            None
        }
        (Ok(_), Err(b)) => panic!("{what}: litestd succeeded, std failed: {b}"),
        (Err(a), Ok(_)) => {
            panic!("{what}: litestd failed: {a:?}, std succeeded")
        }
    }
}
