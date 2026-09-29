//! litestd's `process::Command` on Windows, compared with std's on the same
//! commands: the command line and program a child gets, its environment and
//! working directory, pipes, exit codes, errors, handle inheritance, and the
//! `os::windows::process` extensions.
//!
//! Children are this test binary, which reports what it received, or a
//! suspended copy of it, whose command line is read from the kernel; see
//! `command_windows_util`.

#![cfg(all(windows, feature = "command"))]
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    reason = "helpers, like tests, fail on errors"
)]

mod command_windows_util;

use core::time::Duration;
use std::{
    ffi::{OsStr, OsString},
    os::windows::{
        ffi::{OsStrExt as _, OsStringExt as _},
        process::ExitStatusExt as _,
    },
    sync::mpsc,
    thread,
};

use command_windows_util::{
    TempDir, exe, hex, lite, lite_child, lite_snapshot, payload, real,
    report_lines, same, std_child, std_snapshot, under_wine,
};
use litestd::{
    io::{Read as _, Write as _},
    os::windows::{
        io::{
            AsHandle as _, AsRawHandle as _, FromRawHandle as _,
            IntoRawHandle as _, OwnedHandle,
        },
        process::{CommandExt as _, ExitStatusExt as _},
    },
    process::{Command, Stdio},
};
use windows_sys::Win32::{
    Foundation::{GetHandleInformation, HANDLE_FLAG_INHERIT},
    System::Threading::GetProcessId,
};

#[test]
fn child_main() {
    command_windows_util::child_main();
}

/// Arguments that exercise the quoting: whitespace, quotes, backslashes
/// before quotes and at the end, empty strings, characters that shells
/// treat specially, and non-ASCII text.
fn corpus() -> Vec<OsString> {
    [
        "",
        " ",
        "  ",
        "\t",
        "a",
        "a b",
        "a\tb",
        " a ",
        "a  b",
        "\"",
        "\"\"",
        "a\"b",
        "\"a b\"",
        "a\"",
        "\"a",
        "\\",
        "\\\\",
        "a\\",
        "a\\\\",
        "a b\\",
        "a b\\\\",
        "\\\"",
        "a\\\"b",
        "a\\\\\"b",
        "a\\\\\\\"b",
        "\\\\\\\"",
        "a\\b",
        "\\\\server\\share",
        "C:\\Program Files\\x",
        "a\nb",
        "a\u{b}b",
        "a\rb",
        "%PATH%",
        "^&|<>()",
        "!x!",
        "--",
        "-",
        "/c",
        "\u{e9}",
        "\u{65e5}\u{672c}\u{8a9e}",
        "\u{1f600}",
        "a\u{85}b",
        "\"\\\"\"",
    ]
    .map(OsString::from)
    .into()
}

/// Arguments with unpaired surrogates, which a running child's test harness
/// rejects, so that only suspended children get them.
fn surrogates() -> [OsString; 3] {
    [
        OsString::from_wide(&[0xD800]),
        OsString::from_wide(&[0x61, 0xDC00, 0x62]),
        OsString::from_wide(&[0xDBFF, 0x20, 0x22, 0xDC00]),
    ]
}

/// Runs this binary in `mode` with `args`, through litestd and std.
fn run_both(
    mode: &str,
    args: &[OsString],
) -> (litestd::process::Output, std::process::Output) {
    let mut lite_cmd = lite_child(mode);
    let mut std_cmd = std_child(mode);
    for arg in args {
        lite_cmd.arg(lite(arg));
        std_cmd.arg(arg);
    }
    let lite_out = lite_cmd.output().unwrap();
    let std_out = std_cmd.output().unwrap();
    assert!(lite_out.status.success(), "{lite_out:?}");
    assert!(std_out.status.success(), "{std_out:?}");
    (lite_out, std_out)
}

#[test]
fn arguments_reach_the_child_unchanged() {
    let args = corpus();
    let (lite_out, std_out) = run_both("args", &args);
    let lines = report_lines(&lite_out.stdout);
    assert_eq!(lines, report_lines(&std_out.stdout));
    let expected: Vec<String> =
        args.iter().map(|a| hex(a.encode_wide())).collect();
    assert_eq!(lines, expected);
}

#[test]
fn command_line_is_std_s() {
    let args = corpus();
    let (lite_out, std_out) = run_both("cmdline", &args);
    assert_eq!(
        report_lines(&lite_out.stdout),
        report_lines(&std_out.stdout)
    );
    // Each argument alone as well, from a suspended child.
    for arg in args.iter().chain(&surrogates()) {
        let mut lite_cmd = lite_child("args");
        let mut std_cmd = std_child("args");
        let a = lite_snapshot(lite_cmd.arg(lite(arg))).unwrap();
        let b = std_snapshot(std_cmd.arg(arg)).unwrap();
        assert_eq!(a, b, "{arg:?}");
    }
}

#[test]
fn raw_arguments_are_appended_as_they_are() {
    let raws = [
        "a b",
        "\"a b\" c",
        "",
        "x\\\"y",
        "  ",
        "\u{e9}",
        "\"",
        "a\\",
    ];
    let mut lite_cmd = lite_child("args");
    let mut std_cmd = std_child("args");
    for raw in raws {
        use std::os::windows::process::CommandExt as _;
        lite_cmd.arg("regular one").raw_arg(raw);
        std_cmd.arg("regular one").raw_arg(raw);
    }
    lite_cmd.arg("last");
    std_cmd.arg("last");
    let a = lite_cmd.output().unwrap();
    let b = std_cmd.output().unwrap();
    assert_eq!(report_lines(&a.stdout), report_lines(&b.stdout));
    assert_eq!(
        lite_snapshot(&mut lite_cmd).unwrap(),
        std_snapshot(&mut std_cmd).unwrap()
    );
}

#[test]
fn debug_prints_as_std() {
    use std::os::windows::process::CommandExt as _;
    let mut lite_cmd = Command::new("prog name");
    let mut std_cmd = std::process::Command::new("prog name");
    lite_cmd.args(["a", "b c", "\"q\""]).raw_arg("raw \"text\"");
    std_cmd.args(["a", "b c", "\"q\""]).raw_arg("raw \"text\"");
    assert_eq!(format!("{lite_cmd:?}"), format!("{std_cmd:?}"));
}

/// A copy of this binary as `name` in `dir`, for the program searches.
fn copy_exe(dir: &TempDir, name: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::copy(exe(), &path).unwrap();
    path
}

/// Asserts that litestd and std start the same program with the same
/// command line for `program`, with `path` as the child's `PATH` if set,
/// or fail alike.
fn same_program(program: &OsStr, path: Option<&OsStr>) {
    let mut lite_cmd = Command::new(lite(program));
    let mut std_cmd = std::process::Command::new(program);
    if let Some(path) = path {
        lite_cmd.env("PATH", lite(path));
        std_cmd.env("PATH", path);
    }
    let what = format!("{program:?} with PATH {path:?}");
    if let Some((a, b)) = same(
        &what,
        lite_snapshot(&mut lite_cmd),
        std_snapshot(&mut std_cmd),
    ) {
        assert_eq!(a, b, "{what}");
    }
}

#[test]
fn programs_resolve_as_with_std() {
    let dir = TempDir::new("resolve");
    let child = copy_exe(&dir, "child.exe");
    copy_exe(&dir, "dotted.v2");
    let dir_str = dir.path().as_os_str().to_owned();
    let mut verbatim = OsString::from(r"\\?\");
    verbatim.push(&child);
    let slashes: String = child.to_str().unwrap().replace('\\', "/");
    let mut quoted_path = OsString::from("\"");
    quoted_path.push(&dir_str);
    quoted_path.push("\";C:\\nowhere");
    let without_exe = dir.join("child");
    let missing = dir.join("missing.exe");
    let this = exe();
    let this_name = this.file_name().unwrap();
    let cases: [(&OsStr, Option<&OsStr>); 13] = [
        (child.as_os_str(), None),
        (without_exe.as_os_str(), None),
        (verbatim.as_os_str(), None),
        (OsStr::new(&slashes), None),
        (OsStr::new("child"), Some(&dir_str)),
        (OsStr::new("child.exe"), Some(&dir_str)),
        (OsStr::new("CHILD.EXE"), Some(&dir_str)),
        (OsStr::new("child"), Some(&quoted_path)),
        (OsStr::new("dotted.v2"), Some(&dir_str)),
        (OsStr::new("dotted"), Some(&dir_str)),
        // The directory of the parent's program comes before system ones.
        (this_name, Some(OsStr::new(""))),
        (OsStr::new("litestd-missing-program"), Some(&dir_str)),
        (missing.as_os_str(), None),
    ];
    for (program, path) in cases {
        same_program(program, path);
    }
}

#[test]
fn bad_programs_fail_as_with_std() {
    let dir = TempDir::new("bad");
    let trailing = format!("{}\\", dir.path().display());
    let cases: [&str; 6] = [
        "",
        &trailing,
        "a/",
        "litestd-missing-program",
        "litestd\0missing",
        "C:\\litestd\0missing",
    ];
    for program in cases {
        let a = Command::new(program).spawn();
        let b = std::process::Command::new(program).spawn();
        same(&format!("{program:?}"), a.map(|_| ()), b.map(|_| ()));
    }
}

/// Changes a litestd command, and the same for std.
type SetLite = fn(&mut Command);
type SetStd = fn(&mut std::process::Command);

#[test]
fn nul_bytes_fail_as_with_std() {
    use std::os::windows::process::CommandExt as _;
    let mut lite_cmd = lite_child("args");
    let mut std_cmd = std_child("args");
    same(
        "arg",
        lite_cmd.arg("a\0b").spawn().map(|_| ()),
        std_cmd.arg("a\0b").spawn().map(|_| ()),
    );
    let checks: [(&str, SetLite, SetStd); 5] = [
        (
            "raw",
            |c| {
                c.raw_arg("a\0");
            },
            |c| {
                c.raw_arg("a\0");
            },
        ),
        (
            "key",
            |c| {
                c.env("K\0", "v");
            },
            |c| {
                c.env("K\0", "v");
            },
        ),
        (
            "value",
            |c| {
                c.env("K", "v\0");
            },
            |c| {
                c.env("K", "v\0");
            },
        ),
        (
            "cwd",
            |c| {
                c.current_dir("a\0");
            },
            |c| {
                c.current_dir("a\0");
            },
        ),
        // A removed variable is not checked, in std either.
        (
            "removed",
            |c| {
                c.env_remove("K\0");
            },
            |c| {
                c.env_remove("K\0");
            },
        ),
    ];
    for (what, set_lite, set_std) in checks {
        let mut lite_cmd = lite_child("exit:0");
        let mut std_cmd = std_child("exit:0");
        lite_cmd.stdout(Stdio::null());
        std_cmd.stdout(std::process::Stdio::null());
        set_lite(&mut lite_cmd);
        set_std(&mut std_cmd);
        let a = lite_cmd.status().map(|s| s.code());
        let b = std_cmd.status().map(|s| s.code());
        if let Some((a, b)) = same(what, a, b) {
            assert_eq!(a, b, "{what}");
        }
    }
}

/// The child's environment block, as litestd and std start it.
fn env_blocks(
    configure_lite: impl Fn(&mut Command),
    configure_std: impl Fn(&mut std::process::Command),
) -> (Vec<String>, Vec<String>) {
    let mut lite_cmd = lite_child("env");
    let mut std_cmd = std_child("env");
    configure_lite(&mut lite_cmd);
    configure_std(&mut std_cmd);
    let a = lite_cmd.output().unwrap();
    let b = std_cmd.output().unwrap();
    (report_lines(&a.stdout), report_lines(&b.stdout))
}

#[test]
fn environments_are_std_s() {
    macro_rules! both {
        ($($method:ident($($arg:expr),*)).*) => {
            env_blocks(
                |c| { c$(.$method($($arg),*))*; },
                |c| { c$(.$method($($arg),*))*; },
            )
        };
    }
    let cases = [
        both!(env("LITESTD_A", "1")),
        both!(env("LITESTD_B", "")),
        both!(env("path", "C:\\x")),
        both!(env("Path", "C:\\x").env("PATH", "C:\\y")),
        both!(env("litestd_c", "1").env("LITESTD_C", "2")),
        both!(env_remove("TEMP").env_remove("litestd_none")),
        both!(env_remove("Path").env("PATH", "C:\\z")),
        both!(env_clear()),
        both!(env_clear().env("ZZ", "1").env("aa", "2").env("_", "3")),
        both!(env_clear().env("A=B", "c")),
        both!(env("\u{e9}", "\u{fc}").env("\u{65e5}\u{672c}", "\u{8a9e}")),
        both!(env("LITESTD_D", "1").env_remove("LITESTD_D")),
        both!(env_clear().env("K", "v").env_remove("K")),
    ];
    for (i, (a, b)) in cases.into_iter().enumerate() {
        assert_eq!(a, b, "case {i}");
    }
    // Inherited unchanged.
    let (a, b) = env_blocks(|_| {}, |_| {});
    assert_eq!(a, b);
    assert_ne!(a.len(), 0);
}

#[test]
fn working_directories_are_std_s() {
    let dir = TempDir::new("cwd");
    std::fs::create_dir(dir.join("sub dir")).unwrap();
    let sub = dir.join("sub dir");
    let mut verbatim = OsString::from(r"\\?\");
    verbatim.push(&sub);
    let mut trailing = sub.clone().into_os_string();
    trailing.push("\\");
    let missing = dir.join("missing");
    let dirs: [&OsStr; 5] = [
        sub.as_os_str(),
        &verbatim,
        &trailing,
        OsStr::new(".."),
        missing.as_os_str(),
    ];
    for cwd in dirs {
        let a = lite_child("cwd").current_dir(lite(cwd)).output();
        let b = std_child("cwd").current_dir(cwd).output();
        if let Some((a, b)) = same(&format!("{cwd:?}"), a, b) {
            assert_eq!(report_lines(&a.stdout), report_lines(&b.stdout));
        }
    }
    let out = lite_child("cwd").current_dir(lite(&sub)).output().unwrap();
    let expected = hex(sub.as_os_str().encode_wide());
    assert_eq!(report_lines(&out.stdout), [expected]);
}

#[test]
fn exit_statuses_are_std_s() {
    for code in [0_u32, 1, 2, 255, 256, 0x7FFF_FFFF, 0xC000_0005, u32::MAX] {
        let mode = format!("exit:{code}");
        let a = lite_child(&mode).stdout(Stdio::null()).status().unwrap();
        let b = std_child(&mode)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert_eq!(a.code(), b.code(), "{code:#x}");
        assert_eq!(a.success(), b.success(), "{code:#x}");
        assert_eq!(a.to_string(), b.to_string(), "{code:#x}");
        assert_eq!(format!("{a:?}"), format!("{b:?}"), "{code:#x}");
        let raw = litestd::process::ExitStatus::from_raw(code);
        let std_raw = std::process::ExitStatus::from_raw(code);
        assert_eq!(raw, a);
        assert_eq!(raw.to_string(), std_raw.to_string());
    }
    let default = litestd::process::ExitStatus::default();
    assert_eq!(default.code(), Some(0));
    assert!(default.success());
}

#[test]
fn stdin_and_stdout_pipes_carry_data() {
    let mut child = lite_child("echo")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let data: Vec<u8> = (0..=255).cycle().take(200_000).collect();
    let mut stdin = child.stdin.take().unwrap();
    // The child writes back while it reads, so the input goes from another
    // thread than the one that reads the output.
    let writer = {
        let data = data.clone();
        thread::spawn(move || stdin.write_all(&data).unwrap())
    };
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    writer.join().unwrap();
    assert_eq!(payload(&out), data);
    assert!(child.wait().unwrap().success());
}

#[test]
fn output_reads_both_streams_without_deadlock() {
    for size in [0_usize, 1, 5000, 1 << 20] {
        let mode = format!("flood:{size}");
        let out = lite_child(&mode).output().unwrap();
        assert!(out.status.success(), "{size}");
        let stdout = payload(&out.stdout);
        assert_eq!(stdout.len(), size);
        assert!(stdout.iter().all(|&b| b == b'o'));
        assert_eq!(out.stderr.len(), size);
        assert!(out.stderr.iter().all(|&b| b == b'e'));
    }
    let child = lite_child("flood:300000")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(payload(&out.stdout).len(), 300_000);
    assert_eq!(out.stderr.len(), 300_000);
}

#[test]
fn output_leaves_stdin_null_and_stdio_null_discards() {
    let out = lite_child("echo").output().unwrap();
    assert_eq!(payload(&out.stdout), b"");
    let out = lite_child("stderr").stderr(Stdio::null()).output().unwrap();
    assert_eq!(out.stderr, b"");
    assert_eq!(payload(&out.stdout), b"to stdout");
    let out = lite_child("stderr").stdout(Stdio::null()).output().unwrap();
    assert_eq!(out.stdout, b"");
    assert_eq!(out.stderr, b"to stderr");
    let status = lite_child("stderr")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::inherit())
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn a_child_s_output_feeds_another_child() {
    let mut first = lite_child("echo")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let second = lite_child("echo")
        .stdin(Stdio::from(first.stdout.take().unwrap()))
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = first.stdin.take().unwrap();
    stdin.write_all(b"through two children").unwrap();
    drop(stdin);
    let out = second.wait_with_output().unwrap();
    // The second child echoes the first one's report line and payload.
    assert_eq!(payload(payload(&out.stdout)), b"through two children");
    assert!(first.wait().unwrap().success());
}

#[test]
fn a_file_or_handle_can_be_a_stream() {
    let dir = TempDir::new("file");
    let path = dir.join("out.txt");
    let file = std::fs::File::create(&path).unwrap();
    // SAFETY: the handle is std's to give away, and nothing else owns it.
    let handle = unsafe {
        OwnedHandle::from_raw_handle(
            std::os::windows::io::IntoRawHandle::into_raw_handle(file),
        )
    };
    let status = lite_child("stderr")
        .stdout(Stdio::from(handle))
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    let written = std::fs::read(&path).unwrap();
    assert_eq!(payload(&written), b"to stdout");
    let file = std::fs::File::create(&path).unwrap();
    // SAFETY: as above.
    let stdio = unsafe {
        Stdio::from_raw_handle(
            std::os::windows::io::IntoRawHandle::into_raw_handle(file),
        )
    };
    let mut cmd = lite_child("stderr");
    cmd.stderr(stdio).stdout(Stdio::null());
    // The command keeps the handle and duplicates it for each child.
    assert!(cmd.status().unwrap().success());
    assert!(cmd.status().unwrap().success());
    assert_eq!(std::fs::read(&path).unwrap(), b"to stderrto stderr");
}

#[cfg(feature = "fs")]
#[test]
fn a_litestd_file_can_be_a_stream() {
    let dir = TempDir::new("litefile");
    let path = dir.join("in.txt");
    std::fs::write(&path, b"from a file").unwrap();
    let file = litestd::fs::File::open(lite(&path)).unwrap();
    let out = lite_child("echo").stdin(file).output().unwrap();
    assert_eq!(payload(&out.stdout), b"from a file");
}

#[test]
fn kill_try_wait_and_wait() {
    let mut child = lite_child("sleep").stdout(Stdio::null()).spawn().unwrap();
    assert!(child.try_wait().unwrap().is_none());
    // SAFETY: the handle is the child's, open while it lives.
    let pid = unsafe { GetProcessId(child.as_raw_handle()) };
    assert_eq!(pid, child.id());
    assert_ne!(pid, litestd::process::id());
    child.kill().unwrap();
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(1));
    assert_eq!(child.try_wait().unwrap(), Some(status));
    assert_eq!(child.wait().unwrap(), status);
    // Killing a process that has exited is not an error, as in std.
    child.kill().unwrap();
    let mut std_child = std_child("exit:3")
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std_child.wait().unwrap();
    std_child.kill().unwrap();
}

#[test]
fn handles_for_the_parent_are_not_inheritable() {
    let mut child = lite_child("echo")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let handles = [
        child.as_raw_handle(),
        child.stdin.as_ref().unwrap().as_raw_handle(),
        child.stdout.as_ref().unwrap().as_raw_handle(),
        child.stderr.as_ref().unwrap().as_raw_handle(),
    ];
    for handle in handles {
        let mut flags = 0;
        // SAFETY: the handle is open, and `flags` is writable.
        let ok = unsafe { GetHandleInformation(handle, &raw mut flags) };
        assert_ne!(ok, 0);
        assert_eq!(flags & HANDLE_FLAG_INHERIT, 0);
    }
    drop(child.stdin.take());
    child.wait().unwrap();
}

#[test]
fn a_later_child_does_not_keep_a_pipe_open() {
    let mut first = lite_child("echo")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    // Started while the first child's pipes exist; had it inherited the
    // first child's end of its stdout, reading that would never end.
    let mut sleeper = lite_child("sleep")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    drop(first.stdin.take());
    let mut stdout = first.stdout.take().unwrap();
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let mut out = Vec::new();
        let result = stdout.read_to_end(&mut out).map(|_| out);
        done.send(result.is_ok()).unwrap();
    });
    let read = finished.recv_timeout(Duration::from_secs(30));
    sleeper.kill().unwrap();
    sleeper.wait().unwrap();
    assert_eq!(read, Ok(true));
    first.wait().unwrap();
}

#[test]
fn threads_spawn_at_once() {
    let threads: Vec<_> = (0..8)
        .map(|i| {
            thread::spawn(move || {
                for j in 0..4 {
                    let arg = format!("thread {i} child {j}");
                    let out = lite_child("args").arg(&arg).output().unwrap();
                    let expected = hex(OsStr::new(&arg).encode_wide());
                    assert_eq!(report_lines(&out.stdout), [expected]);
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
}

#[test]
fn handle_conversions() {
    let mut child = lite_child("echo")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    assert_eq!(stdin.as_handle().as_raw_handle(), stdin.as_raw_handle());
    let owned = OwnedHandle::from(stdin);
    let mut stdin = litestd::process::ChildStdin::from(owned);
    stdin.write_all(b"round trip").unwrap();
    // SAFETY: `into_raw_handle` hands over the open pipe handle.
    let stdin = unsafe {
        litestd::process::ChildStdin::from(OwnedHandle::from_raw_handle(
            stdin.into_raw_handle(),
        ))
    };
    drop(stdin);
    let stdout = child.stdout.take().unwrap();
    let mut stdout =
        litestd::process::ChildStdout::from(OwnedHandle::from(stdout));
    let mut out = Vec::new();
    stdout.read_to_end(&mut out).unwrap();
    assert_eq!(payload(&out), b"round trip");
    let raw = child.as_raw_handle();
    let handle = OwnedHandle::from(child);
    assert_eq!(handle.as_raw_handle(), raw);
    // SAFETY: the handle is the child's process handle, open.
    let status = unsafe {
        windows_sys::Win32::System::Threading::WaitForSingleObject(raw, 60_000)
    };
    assert_eq!(status, 0);
    let child = lite_child("exit:4").stdout(Stdio::null()).spawn().unwrap();
    let raw = child.into_raw_handle();
    // SAFETY: `into_raw_handle` handed over the process handle.
    drop(unsafe { OwnedHandle::from_raw_handle(raw) });
}

#[test]
fn creation_flags_reach_create_process() {
    /// `CREATE_NO_WINDOW` from `winbase.h`.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = lite_child("args")
        .creation_flags(CREATE_NO_WINDOW)
        .arg("flags")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        report_lines(&out.stdout),
        [hex(OsStr::new("flags").encode_wide())]
    );
}

#[test]
fn getters_report_the_configuration() {
    let mut cmd = Command::new("prog");
    cmd.arg("a").raw_arg("b c").env("K", "v").env_remove("R");
    cmd.current_dir("dir");
    assert_eq!(real(cmd.get_program()), "prog");
    let args: Vec<OsString> = cmd.get_args().map(real).collect();
    assert_eq!(args, ["a", "b c"]);
    let envs: Vec<(OsString, Option<OsString>)> = cmd
        .get_envs()
        .map(|(k, v)| (real(k), v.map(real)))
        .collect();
    assert_eq!(envs, [("K".into(), Some("v".into())), ("R".into(), None)]);
    assert_eq!(
        cmd.get_current_dir().map(|d| real(d.as_os_str())),
        Some("dir".into())
    );
}

#[test]
fn stdout_and_stderr_are_separate() {
    let a = lite_child("stderr").output().unwrap();
    let b = std_child("stderr").output().unwrap();
    assert_eq!(payload(&a.stdout), payload(&b.stdout));
    assert_eq!(a.stderr, b.stderr);
}

#[test]
fn unusual_program_paths_resolve_as_with_std() {
    let dir = TempDir::new("paths");
    let unicode_dir = dir.join("d\u{ed}r \u{fc} \u{65e5}\u{672c}");
    std::fs::create_dir(&unicode_dir).unwrap();
    let unicode = unicode_dir.join("\u{e7}hild.exe");
    std::fs::copy(exe(), &unicode).unwrap();
    // Paths of 248 to 260 units, NUL included, which std makes verbatim,
    // and a longer one, which it leaves alone.
    let dir_len = dir.path().as_os_str().encode_wide().count();
    let near_limit = dir.join(&"n".repeat(249 - dir_len - 7)).join("c.exe");
    std::fs::create_dir(near_limit.parent().unwrap()).unwrap();
    std::fs::copy(exe(), &near_limit).unwrap();
    assert_eq!(near_limit.as_os_str().encode_wide().count() + 1, 250);
    let long_dir = dir.join(&"l".repeat(150)).join("m".repeat(150));
    std::fs::create_dir_all(&long_dir).unwrap();
    let long = long_dir.join("c.exe");
    std::fs::copy(exe(), &long).unwrap();
    let child = copy_exe(&dir, "child.exe");
    let child_str = child.to_str().unwrap();
    // `C:` followed by a relative path, and a path from the root of the
    // current drive.
    let drive_relative = format!("{}{}", &child_str[..2], &child_str[3..]);
    let root_relative = &child_str[2..];
    let dir_str = dir.path().as_os_str();
    let cases: [(&OsStr, Option<&OsStr>); 6] = [
        (unicode.as_os_str(), None),
        (OsStr::new("\u{e7}hild"), Some(unicode_dir.as_os_str())),
        (near_limit.as_os_str(), None),
        (OsStr::new(&drive_relative), None),
        (OsStr::new(root_relative), None),
        (OsStr::new("child"), Some(dir_str)),
    ];
    for (program, path) in cases {
        same_program(program, path);
    }
    // Wine's `CreateProcessW` builds the image name of a path longer than
    // `MAX_PATH` from memory it never initialized, so that the error, if
    // any, changes from one call to the next.
    if under_wine() {
        let lite_result = lite_snapshot(&mut Command::new(lite(&long)));
        let std_result = std_snapshot(&mut std::process::Command::new(&long));
        assert_eq!(
            lite_result.is_ok(),
            std_result.is_ok(),
            "{long:?}: {lite_result:?} vs {std_result:?}"
        );
    } else {
        same_program(long.as_os_str(), None);
    }
    // A relative program is found from the parent's working directory,
    // not the child's, and the child's `PATH` may be spelled in any case.
    let mut lite_cmd = Command::new(r".\child.exe");
    let mut std_cmd = std::process::Command::new(r".\child.exe");
    lite_cmd.current_dir(lite(dir.path()));
    std_cmd.current_dir(dir.path());
    same(
        "relative",
        lite_snapshot(&mut lite_cmd),
        std_snapshot(&mut std_cmd),
    );
    let mut lite_cmd = Command::new("child");
    let mut std_cmd = std::process::Command::new("child");
    lite_cmd.env("Path", lite(dir_str));
    std_cmd.env("Path", dir_str);
    let (a, b) = same(
        "Path",
        lite_snapshot(&mut lite_cmd),
        std_snapshot(&mut std_cmd),
    )
    .unwrap();
    assert_eq!(a, b);
    assert!(a.image.ends_with("child.exe"), "{a:?}");
}

#[test]
fn the_parent_s_path_is_searched_last() {
    let dir = TempDir::new("parent-path");
    copy_exe(&dir, "parentonly.exe");
    let this = exe();
    let this_name = this.file_name().unwrap().to_str().unwrap();
    copy_exe(&dir, this_name);
    copy_exe(&dir, "cmd.exe");
    // The child resolves each name with `dir` as its own `PATH`: found
    // there only if not in the child's directory or the system ones.
    let resolve = |name: &str| {
        let out = lite_child(&format!("resolve:{name}"))
            .env("PATH", lite(dir.path()))
            .output()
            .unwrap();
        assert!(out.status.success(), "{name}: {out:?}");
        report_lines(&out.stdout).join("\n").to_lowercase()
    };
    let found = resolve("parentonly");
    assert!(found.ends_with("parentonly.exe"), "{found}");
    assert!(found.contains("litestd-cmd-parent-path"), "{found}");
    let found = resolve(this_name.trim_end_matches(".exe"));
    assert!(!found.contains("litestd-cmd-parent-path"), "{found}");
    let found = resolve("cmd");
    assert!(found.ends_with("\\system32\\cmd.exe"), "{found}");
}

#[test]
fn threads_write_to_one_stdin_at_once() {
    let mut child = lite_child("echo")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let reader = thread::spawn(move || {
        let mut out = Vec::new();
        stdout.read_to_end(&mut out).unwrap();
        out
    });
    thread::scope(|s| {
        for i in 0..4 {
            let stdin = &stdin;
            s.spawn(move || {
                let chunk = [b'a' + i; 1000];
                for _ in 0..50 {
                    (&*stdin).write_all(&chunk).unwrap();
                }
            });
        }
    });
    drop(stdin);
    let out = reader.join().unwrap();
    let data = payload(&out);
    assert_eq!(data.len(), 4 * 50 * 1000);
    for i in 0..4 {
        #[allow(clippy::naive_bytecount, reason = "no crate for one test")]
        let count = data.iter().filter(|&&b| b == b'a' + i).count();
        assert_eq!(count, 50 * 1000);
    }
    assert!(child.wait().unwrap().success());
}

#[test]
fn a_child_writes_into_another_child_s_input() {
    let mut reader = lite_child("echo")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = reader.stdin.take().unwrap();
    // Once the writer has exited and its command is gone, the relay closes
    // the reader's input.
    let status = lite_child("stderr")
        .stdout(Stdio::from(stdin))
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    let out = reader.wait_with_output().unwrap();
    assert_eq!(payload(payload(&out.stdout)), b"to stdout");
}

#[test]
fn spawning_leaks_no_handles() {
    let out = lite_child("leaks").output().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(report_lines(&out.stdout), ["0"], "{out:?}");
}

#[test]
fn an_extended_startup_info_flag_is_harmless() {
    /// `EXTENDED_STARTUPINFO_PRESENT` from `winbase.h`, which promises a
    /// `STARTUPINFOEXW`: litestd's has no attribute list.
    const EXTENDED_STARTUPINFO_PRESENT: u32 = 0x0008_0000;
    let result = lite_child("exit:5")
        .stdout(Stdio::null())
        .creation_flags(EXTENDED_STARTUPINFO_PRESENT)
        .status();
    match result {
        Ok(status) => assert_eq!(status.code(), Some(5)),
        Err(e) => assert_eq!(e.kind(), litestd::io::ErrorKind::InvalidInput),
    }
}

#[cfg(feature = "stdio")]
#[test]
fn this_process_s_streams_can_be_a_child_s() {
    for stream in ["out", "err"] {
        let run = |library: &str| {
            let mode = format!("redirect:{library}:{stream}");
            let out = lite_child(&mode).output().unwrap();
            assert!(out.status.success(), "{mode}: {out:?}");
            out
        };
        let (a, b) = (run("lite"), run("std"));
        assert_eq!(a.stdout, b.stdout, "{stream}");
        assert_eq!(a.stderr, b.stderr, "{stream}");
        // The grandchild's stdout went where it was sent.
        let target = if stream == "out" {
            &a.stdout
        } else {
            &a.stderr
        };
        let text = String::from_utf8_lossy(target);
        assert!(text.ends_with("to stdout"), "{stream}: {text:?}");
    }
}
