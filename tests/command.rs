//! `litestd::process::Command` against `std::process::Command`: each test
//! has both spawn the same child, most often this test binary acting as a
//! helper, and compares what they report.
//!
//! Every child runs in a scratch directory of its own, unless the test is
//! about the working directory. Miri cannot spawn processes, so this file is
//! skipped under it.

#![cfg(all(feature = "command", not(miri)))]
#![allow(clippy::unwrap_used, reason = "a failure fails the test")]

use core::sync::atomic::{AtomicUsize, Ordering};
use std::{
    ffi::OsString,
    io::{Read as _, Write as _},
    path::PathBuf,
    thread,
};

use litestd::{
    io::{Read as _, Write as _},
    process as lite,
};

/// The environment variable that turns this binary into a helper child.
const MODE: &str = "LITESTD_COMMAND_CHILD";

/// A directory of its own for one test, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        let dir = format!("litestd-command-{name}-{}-{n}", std::process::id());
        let path = std::env::temp_dir().join(dir);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The same command, built for litestd and for std.
struct Pair {
    lite: lite::Command,
    std: std::process::Command,
}

impl Pair {
    /// This test binary as a helper child running `mode`, in `dir`.
    fn child(mode: &str, dir: &Scratch) -> Self {
        let exe = std::env::current_exe().unwrap();
        let mut pair = Self {
            lite: lite::Command::new(exe.to_str().unwrap()),
            std: std::process::Command::new(&exe),
        };
        let harness = ["--exact", "child", "--nocapture", "--test-threads=1"];
        pair.args(harness).args(["-q", "--"]).env(MODE, mode);
        pair.current_dir(&dir.0);
        pair
    }

    fn args<I: IntoIterator<Item = S> + Clone, S: AsRef<std::ffi::OsStr>>(
        &mut self,
        args: I,
    ) -> &mut Self {
        self.lite
            .args(args.clone().into_iter().map(|a| to_os(a.as_ref())));
        self.std.args(args);
        self
    }

    fn env(&mut self, key: &str, value: &str) -> &mut Self {
        self.lite.env(key, value);
        self.std.env(key, value);
        self
    }

    fn current_dir(&mut self, dir: &std::path::Path) -> &mut Self {
        self.lite.current_dir(dir.to_str().unwrap());
        self.std.current_dir(dir);
        self
    }

    /// Runs both with `output` and checks that they agree.
    fn same_output(&mut self) -> std::process::Output {
        let lite = self.lite.output().unwrap();
        let std = self.std.output().unwrap();
        assert_same_output(&lite, &std);
        std
    }
}

fn to_os(s: &std::ffi::OsStr) -> litestd::ffi::OsString {
    litestd::ffi::OsString::from(s.to_str().unwrap())
}

fn assert_same_status(lite: lite::ExitStatus, std: std::process::ExitStatus) {
    assert_eq!(lite.code(), std.code(), "{lite} vs {std}");
    assert_eq!(lite.success(), std.success());
    assert_eq!(lite.to_string(), std.to_string());
    assert_eq!(format!("{lite:?}"), format!("{std:?}"));
}

fn assert_same_output(lite: &lite::Output, std: &std::process::Output) {
    assert_same_status(lite.status, std.status);
    assert_eq!(lite.stdout, std.stdout, "stdout of {std:?}");
    assert_eq!(lite.stderr, std.stderr, "stderr of {std:?}");
}

/// The helper side of the tests; a no-op unless `MODE` is set.
#[test]
fn child() {
    let Ok(mode) = std::env::var(MODE) else {
        return;
    };
    let args: Vec<String> =
        std::env::args().skip_while(|a| a != "--").skip(1).collect();
    let mut out = std::io::stdout().lock();
    match mode.as_str() {
        "exit" => std::process::exit(args[0].parse().unwrap()),
        "args" => {
            for arg in &args {
                writeln!(out, "[{arg}]").unwrap();
            }
        }
        "env" => {
            for (key, value) in std::env::vars_os() {
                writeln!(out, "{key:?}={value:?}").unwrap();
            }
        }
        "cwd" => {
            writeln!(out, "{:?}", std::env::current_dir().unwrap()).unwrap();
        }
        "echo" => {
            // Copies stdin to stdout, and its length to stderr.
            let mut input = Vec::new();
            std::io::stdin().read_to_end(&mut input).unwrap();
            out.write_all(&input).unwrap();
            eprint!("{}", input.len());
        }
        "spew" => spew(args[0].parse().unwrap()),
        "sleep" => thread::sleep(core::time::Duration::from_secs(100)),
        #[cfg(feature = "stdio")]
        "redirect" => {
            // A grandchild whose stdout goes to this process's stderr, and
            // the other way round, in this helper's directory, which the
            // parent test removes.
            let dir = core::mem::ManuallyDrop::new(Scratch(
                std::env::current_dir().unwrap(),
            ));
            let mut cmd = Pair::child("args", &dir);
            cmd.args(["via", "stderr"]);
            let status = if args.first().is_some_and(|a| a == "std") {
                let std =
                    cmd.std.stdout(std::io::stderr()).stderr(std::io::stdout());
                std.status().unwrap().success()
            } else {
                let lite = cmd.lite.stdout(litestd::io::stderr());
                lite.stderr(litestd::io::stdout())
                    .status()
                    .unwrap()
                    .success()
            };
            assert!(status);
        }
        other => panic!("unknown mode {other}"),
    }
    out.flush().unwrap();
    drop(out);
    std::process::exit(0);
}

/// Writes `len` bytes to stdout and to stderr in alternating chunks, which
/// deadlocks a parent that drains one pipe before the other.
fn spew(len: usize) {
    let (mut out, mut err) =
        (std::io::stdout().lock(), std::io::stderr().lock());
    let chunk: Vec<u8> = (0..8192u32).map(|i| (i % 251) as u8).collect();
    let mut left = len;
    while left > 0 {
        let n = left.min(chunk.len());
        out.write_all(&chunk[..n]).unwrap();
        err.write_all(&chunk[..n]).unwrap();
        left -= n;
    }
}

/// `io::stdout()` and `io::stderr()` as a child's streams mean this
/// process's streams, whichever stream of the child they are given to.
#[cfg(feature = "stdio")]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn standard_handles_redirect_like_std() {
    let dir = Scratch::new("handles");
    let mut lite = Pair::child("redirect", &dir);
    lite.args(["lite"]);
    let lite = lite.std.output().unwrap();
    let mut std = Pair::child("redirect", &dir);
    std.args(["std"]);
    let std = std.std.output().unwrap();
    assert!(lite.status.success() && std.status.success(), "{lite:?}");
    assert!(String::from_utf8_lossy(&lite.stderr).contains("[via]\n[stderr]"));
    assert_eq!(lite.stderr, std.stderr);
}

#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn exit_codes_match_std() {
    let dir = Scratch::new("exit");
    for code in ["0", "1", "7", "255"] {
        let mut pair = Pair::child("exit", &dir);
        pair.args([code]);
        let std = pair.same_output();
        assert_eq!(std.status.code(), Some(code.parse().unwrap()));
        let lite = pair.lite.stdout(lite::Stdio::null()).status().unwrap();
        assert_same_status(lite, std.status);
    }
}

#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn args_reach_the_child_unchanged() {
    let dir = Scratch::new("args");
    let mut pair = Pair::child("args", &dir);
    pair.args([
        "plain",
        "two words",
        "",
        "quote\"d",
        "back\\slash\\",
        "h\u{e9}",
    ]);
    pair.args(["tab\tnew\nline", "trailing\\\\", "\\\"", "%PATH%", "$HOME"]);
    let out = pair.same_output();
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .contains("[two words]")
    );
}

#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn env_changes_match_std() {
    let dir = Scratch::new("env");
    let mut pair = Pair::child("env", &dir);
    let unchanged = pair.same_output();
    pair.env("LITESTD_A", "1").env("LITESTD_B", "two words");
    pair.lite.env_remove("PATH");
    pair.std.env_remove("PATH");
    let changed = pair.same_output();
    assert_ne!(unchanged.stdout, changed.stdout);
    let text = String::from_utf8(changed.stdout).unwrap();
    assert!(text.contains("\"LITESTD_B\"=\"two words\""), "{text}");
    assert!(!text.contains("\"PATH\"="), "{text}");
    // Later changes of the same variable win, and a removal after a clear
    // leaves nothing behind.
    pair.env("LITESTD_A", "3");
    pair.lite
        .env_clear()
        .env(MODE, "env")
        .env_remove("LITESTD_GONE");
    pair.std
        .env_clear()
        .env(MODE, "env")
        .env_remove("LITESTD_GONE");
    pair.env("LITESTD_A", "4");
    #[cfg(windows)]
    for key in ["SystemRoot", "PATHEXT"] {
        if let Some(value) = std::env::var_os(key) {
            pair.env(key, value.to_str().unwrap());
        }
    }
    let cleared = String::from_utf8(pair.same_output().stdout).unwrap();
    assert!(cleared.contains("\"LITESTD_A\"=\"4\""), "{cleared}");
    assert!(!cleared.contains("LITESTD_B"), "{cleared}");
}

#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn current_dir_matches_std() {
    let dir = Scratch::new("cwd");
    let out = Pair::child("cwd", &dir).same_output();
    #[cfg(unix)]
    {
        let cwd = std::fs::canonicalize(&dir.0).unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains(&format!("{cwd:?}")), "{text} vs {cwd:?}");
    }
    #[cfg(not(unix))]
    let _ = out;
}

/// Every combination of piped and null streams gives what std gives.
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn pipes_in_all_combinations() {
    let dir = Scratch::new("pipes");
    for bits in 0..8 {
        let mut pair = Pair::child("echo", &dir);
        let std_cfg = |on: bool| {
            if on {
                std::process::Stdio::piped()
            } else {
                std::process::Stdio::null()
            }
        };
        let lite_cfg = |on: bool| {
            if on {
                lite::Stdio::piped()
            } else {
                lite::Stdio::null()
            }
        };
        let (i, o, e) = (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0);
        pair.std
            .stdin(std_cfg(i))
            .stdout(std_cfg(o))
            .stderr(std_cfg(e));
        pair.lite
            .stdin(lite_cfg(i))
            .stdout(lite_cfg(o))
            .stderr(lite_cfg(e));
        let mut std = pair.std.spawn().unwrap();
        let mut lite = pair.lite.spawn().unwrap();
        assert_eq!(lite.stdin.is_some(), i);
        assert_eq!(lite.stdout.is_some(), o);
        assert_eq!(lite.stderr.is_some(), e);
        if let (Some(l), Some(s)) = (lite.stdin.as_mut(), std.stdin.as_mut()) {
            l.write_all(b"to the child").unwrap();
            s.write_all(b"to the child").unwrap();
        }
        let lite = lite.wait_with_output().unwrap();
        let std = std.wait_with_output().unwrap();
        assert_same_output(&lite, &std);
    }
}

#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn large_outputs_do_not_deadlock() {
    let dir = Scratch::new("large");
    let len = 4 << 20;
    let mut pair = Pair::child("spew", &dir);
    pair.args([len.to_string()]);
    let out = pair.same_output();
    assert!(out.stdout.len() >= len && out.stderr.len() == len);
    // Only one stream captured, through `wait_with_output`.
    pair.lite
        .stderr(lite::Stdio::null())
        .stdout(lite::Stdio::piped());
    pair.std
        .stderr(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped());
    let lite = pair.lite.spawn().unwrap().wait_with_output().unwrap();
    let std = pair.std.spawn().unwrap().wait_with_output().unwrap();
    assert_same_output(&lite, &std);
}

#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn large_input_while_reading_output() {
    let dir = Scratch::new("input");
    let data: Vec<u8> = (0..(3u32 << 20)).map(|i| (i % 253) as u8).collect();
    let mut pair = Pair::child("echo", &dir);
    pair.lite
        .stdin(lite::Stdio::piped())
        .stdout(lite::Stdio::piped());
    pair.lite.stderr(lite::Stdio::piped());
    let mut child = pair.lite.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let writer = {
        let data = data.clone();
        thread::spawn(move || stdin.write_all(&data).unwrap())
    };
    let out = child.wait_with_output().unwrap();
    writer.join().unwrap();
    assert!(out.status.success());
    assert!(out.stdout.ends_with(&data));
    assert_eq!(out.stderr, data.len().to_string().as_bytes());
}

#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn output_gives_the_child_no_stdin() {
    let dir = Scratch::new("nostdin");
    let mut pair = Pair::child("echo", &dir);
    let out = pair.same_output();
    assert_eq!(out.stderr, b"0");
    // An explicit pipe is closed right away.
    pair.lite.stdin(lite::Stdio::piped());
    pair.std.stdin(std::process::Stdio::piped());
    assert_eq!(pair.same_output().stderr, b"0");
}

#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn missing_programs_fail_like_std() {
    let dir = Scratch::new("missing");
    let missing = dir.0.join("no-such-program");
    for program in [missing.to_str().unwrap(), "litestd-no-such-program"] {
        let lite = lite::Command::new(program)
            .current_dir(dir.0.to_str().unwrap())
            .spawn();
        let std = std::process::Command::new(program)
            .current_dir(&dir.0)
            .spawn();
        let (lite, std) = (lite.unwrap_err(), std.unwrap_err());
        assert_eq!(format!("{:?}", lite.kind()), format!("{:?}", std.kind()));
        assert_eq!(lite.kind(), litestd::io::ErrorKind::NotFound);
        assert_eq!(lite.raw_os_error(), std.raw_os_error());
    }
}

#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn a_missing_working_directory_fails_like_std() {
    let dir = Scratch::new("nocwd");
    let gone = dir.0.join("gone");
    let mut pair = Pair::child("exit", &dir);
    pair.args(["0"]).current_dir(&gone);
    let lite = pair.lite.spawn().unwrap_err();
    let std = pair.std.spawn().unwrap_err();
    assert_eq!(format!("{:?}", lite.kind()), format!("{:?}", std.kind()));
    assert_eq!(lite.raw_os_error(), std.raw_os_error());
}

/// A NUL byte in any string fails before anything is created: with
/// `InvalidInput` on Unix, while Windows looks for the program first.
#[test]
fn nul_bytes_fail_like_std() {
    let check = |lite: &mut lite::Command, std: &mut std::process::Command| {
        let (lite, std) = (lite.spawn().unwrap_err(), std.spawn().unwrap_err());
        assert_eq!(format!("{:?}", lite.kind()), format!("{:?}", std.kind()));
        assert_eq!(lite.to_string(), std.to_string());
        if cfg!(unix) {
            assert_eq!(lite.kind(), litestd::io::ErrorKind::InvalidInput);
        }
    };
    check(
        &mut lite::Command::new("a\0b"),
        &mut std::process::Command::new("a\0b"),
    );
    check(
        lite::Command::new("sh").arg("a\0b"),
        std::process::Command::new("sh").arg("a\0b"),
    );
    check(
        lite::Command::new("sh").env("A\0", "b"),
        std::process::Command::new("sh").env("A\0", "b"),
    );
    check(
        lite::Command::new("sh").env("A", "b\0"),
        std::process::Command::new("sh").env("A", "b\0"),
    );
    check(
        lite::Command::new("sh").current_dir("a\0b"),
        std::process::Command::new("sh").current_dir("a\0b"),
    );
}

#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn kill_ends_a_sleeping_child() {
    let dir = Scratch::new("kill");
    let mut pair = Pair::child("sleep", &dir);
    let mut lite = pair.lite.spawn().unwrap();
    let mut std = pair.std.spawn().unwrap();
    assert!(lite.try_wait().unwrap().is_none());
    assert!(lite.id() > 0 && lite.id() != std.id());
    lite.kill().unwrap();
    std.kill().unwrap();
    let status = lite.wait().unwrap();
    assert_same_status(status, std.wait().unwrap());
    assert!(!status.success());
    // The status stays, and killing a reaped child does nothing.
    assert_eq!(lite.wait().unwrap(), status);
    assert_eq!(lite.try_wait().unwrap(), Some(status));
    lite.kill().unwrap();
    std.kill().unwrap();
}

#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn try_wait_reports_an_exit() {
    let dir = Scratch::new("trywait");
    let mut pair = Pair::child("exit", &dir);
    pair.args(["3"]);
    pair.lite.stdout(lite::Stdio::null());
    let mut child = pair.lite.spawn().unwrap();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        thread::sleep(core::time::Duration::from_millis(5));
    };
    assert_eq!(status.code(), Some(3));
    assert_eq!(child.wait().unwrap(), status);
}

#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn a_childs_output_feeds_another_child() {
    let dir = Scratch::new("chain");
    let mut first = Pair::child("spew", &dir);
    first.args(["100000"]);
    first
        .lite
        .stdout(lite::Stdio::piped())
        .stderr(lite::Stdio::null());
    let mut producer = first.lite.spawn().unwrap();
    let mut second = Pair::child("echo", &dir);
    second.lite.stdin(producer.stdout.take().unwrap());
    let out = second.lite.output().unwrap();
    assert!(producer.wait().unwrap().success());
    assert!(out.status.success());
    let len: usize =
        core::str::from_utf8(&out.stderr).unwrap().parse().unwrap();
    assert!(len >= 100_000 && out.stdout.len() >= len);
}

#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn child_pipes_read_and_write_directly() {
    let dir = Scratch::new("direct");
    let mut pair = Pair::child("echo", &dir);
    pair.lite
        .stdin(lite::Stdio::piped())
        .stdout(lite::Stdio::piped());
    pair.lite.stderr(lite::Stdio::null());
    let mut child = pair.lite.spawn().unwrap();
    let stdin = child.stdin.take().unwrap();
    (&stdin).write_all(b"shared ").unwrap();
    let mut stdin = stdin;
    stdin.write_all(b"handle").unwrap();
    stdin.flush().unwrap();
    // Closes the pipe; on wasm32-unknown-unknown, where the test is ignored,
    // it cannot exist.
    #[cfg_attr(target_os = "unknown", allow(clippy::drop_non_drop))]
    drop(stdin);
    let mut text = String::new();
    child
        .stdout
        .as_mut()
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert!(text.ends_with("shared handle"), "{text}");
    assert!(child.wait().unwrap().success());
}

#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "WebAssembly has no child processes"
)]
fn a_command_spawns_repeatedly() {
    let dir = Scratch::new("again");
    let mut pair = Pair::child("args", &dir);
    let first = pair.same_output();
    pair.args(["more"]);
    let second = pair.same_output();
    assert_ne!(first.stdout, second.stdout);
}

#[test]
fn args_iterators_match_std() {
    let mut lite = lite::Command::new("prog");
    let mut std = std::process::Command::new("prog");
    lite.args(["a", "b"]).env("K", "v").env_remove("R");
    std.args(["a", "b"]).env("K", "v").env_remove("R");
    let lite_args: Vec<OsString> = lite
        .get_args()
        .map(|a| a.to_str().unwrap().into())
        .collect();
    assert_eq!(lite_args, std.get_args().collect::<Vec<_>>());
    assert_eq!(lite.get_args().len(), 2);
    assert_eq!(lite.get_envs().len(), std.get_envs().len());
}
