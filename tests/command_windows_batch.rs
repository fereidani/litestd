//! Batch files through litestd's `process::Command`, compared with std's,
//! which has escaped their arguments for `cmd.exe` since CVE-2024-24576:
//! the exact `cmd.exe` command line of a suspended child, the output of the
//! script, and the arguments that std refuses.
//!
//! Wine's `cmd.exe` lacks `%CMDCMDLINE%` and may parse some edge cases
//! differently from Windows', so the command line is read from the kernel,
//! and what a script prints is only compared between litestd and std.

#![cfg(all(windows, feature = "command"))]
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    reason = "helpers, like tests, fail on errors"
)]

mod command_windows_util;

use std::{
    ffi::{OsStr, OsString},
    os::windows::{ffi::OsStringExt as _, process::CommandExt as _},
    path::Path,
};

use command_windows_util::{TempDir, lite, lite_snapshot, same, std_snapshot};
use litestd::{os::windows::process::CommandExt as _, process::Command};

/// A script that prints the arguments it got.
const SCRIPT: &[u8] = b"@echo off\r\necho ARGS=%*\r\necho FIRST=[%1]\r\n";

#[test]
fn child_main() {
    command_windows_util::child_main();
}

/// Arguments that `cmd.exe` would expand, split or execute unless escaped:
/// variables, operators, quotes, carets, trailing backslashes, control
/// characters and non-ASCII text, and some that need no quotes at all.
fn corpus() -> Vec<OsString> {
    [
        "",
        "a",
        "a b",
        "a\"b",
        "\"",
        "\"\"",
        "%PATH%",
        "%%",
        "%",
        "%a%b%",
        "!PATH!",
        "^",
        "a^b",
        "&whoami",
        "a&b",
        "|",
        "a|b",
        "<in",
        ">out",
        "(x)",
        "a,b",
        "a;b",
        "a=b",
        "x\\",
        "x\\\\",
        "\\\"",
        "a\\\"b",
        "a b\\",
        "\t",
        "a\tb",
        "\u{1b}",
        "\u{7f}",
        "\u{85}",
        "\u{9f}",
        "\u{a0}",
        "\u{e9}",
        "\u{65e5}\u{672c}\u{8a9e}",
        "\u{1f600}",
        "#$*+-./:?@\\_",
        "C:\\Program Files\\x",
        "'a'",
        "`a`",
        "~",
        "[x]",
        "{x}",
    ]
    .map(OsString::from)
    .into()
}

/// Writes the script to `dir` as `name`.
fn script(dir: &TempDir, name: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, SCRIPT).unwrap();
    path
}

/// Asserts that litestd and std start `cmd.exe` alike for `program` with
/// `args`, or fail alike.
fn same_start(program: &OsStr, args: &[OsString], path: Option<&Path>) {
    let mut lite_cmd = Command::new(lite(program));
    let mut std_cmd = std::process::Command::new(program);
    for arg in args {
        lite_cmd.arg(lite(arg));
        std_cmd.arg(arg);
    }
    if let Some(path) = path {
        lite_cmd.env("PATH", lite(path));
        std_cmd.env("PATH", path);
    }
    let what = format!("{program:?} {args:?}");
    let lite_result = lite_snapshot(&mut lite_cmd);
    let std_result = std_snapshot(&mut std_cmd);
    if let Some((a, b)) = same(&what, lite_result, std_result) {
        assert_eq!(a, b, "{what}");
        assert!(a.image.to_lowercase().ends_with("\\cmd.exe"), "{what}");
    }
}

#[test]
fn command_lines_are_std_s() {
    let dir = TempDir::new("batch-line");
    let bat = script(&dir, "show.bat");
    let args = corpus();
    for arg in &args {
        same_start(bat.as_os_str(), core::slice::from_ref(arg), None);
    }
    same_start(bat.as_os_str(), &args, None);
    let surrogates = [
        OsString::from_wide(&[0xD800]),
        OsString::from_wide(&[0x61, 0xDC00, 0x25]),
    ];
    same_start(bat.as_os_str(), &surrogates, None);
}

#[test]
fn every_batch_name_form_goes_through_cmd() {
    let dir = TempDir::new("batch-names");
    let bat = script(&dir, "show.bat");
    script(&dir, "other.cmd");
    let path = bat.as_os_str();
    let mut verbatim = OsString::from(r"\\?\");
    verbatim.push(path);
    let with = |suffix: &str| {
        let mut name = path.to_owned();
        name.push(suffix);
        name
    };
    let args = [OsString::from("a b"), OsString::from("%x%")];
    let programs = [
        path.to_owned(),
        dir.join("SHOW.BAT").into_os_string(),
        dir.join("other.cmd").into_os_string(),
        dir.join("OTHER.Cmd").into_os_string(),
        // Windows drops trailing dots and spaces, which std checks the
        // extension after.
        with("."),
        with(" "),
        with(". ."),
        verbatim,
        path.to_string_lossy().replace('\\', "/").into(),
    ];
    for program in &programs {
        same_start(program, &args, None);
    }
    // Found in the child's `PATH`, where no `.exe` is appended to a name
    // with an extension.
    same_start(OsStr::new("show.bat"), &args, Some(dir.path()));
    same_start(OsStr::new("other.cmd"), &args, Some(dir.path()));
}

#[test]
fn refused_arguments_fail_as_with_std() {
    let dir = TempDir::new("batch-refuse");
    let bat = script(&dir, "show.bat");
    let refused = ["a\nb", "\r", "\n", "x\r\ny", "a\0b"];
    for arg in refused {
        let args = [OsString::from("fine"), OsString::from(arg)];
        same_start(bat.as_os_str(), &args, None);
    }
    // A line break in an earlier argument wins over a NUL in a later one,
    // and the other way round.
    let args = [OsString::from("a\n"), OsString::from("b\0")];
    same_start(bat.as_os_str(), &args, None);
    let args = [OsString::from("b\0"), OsString::from("a\n")];
    same_start(bat.as_os_str(), &args, None);
    // A name with a quote cannot exist, and std refuses to quote it.
    let quoted = dir.join("a\"b.bat");
    same_start(quoted.as_os_str(), &[], None);
}

#[test]
fn raw_arguments_reach_cmd_as_they_are() {
    let dir = TempDir::new("batch-raw");
    let bat = script(&dir, "show.bat");
    for raw in ["\"%PATH%\"", "a b", "\"quoted\" and not", "", "x\r\ny"] {
        let mut lite_cmd = Command::new(lite(&bat));
        let mut std_cmd = std::process::Command::new(&bat);
        lite_cmd.arg("first").raw_arg(raw).arg("last%");
        std_cmd.arg("first").raw_arg(raw).arg("last%");
        let a = lite_snapshot(&mut lite_cmd);
        let b = std_snapshot(&mut std_cmd);
        if let Some((a, b)) = same(raw, a, b) {
            assert_eq!(a, b, "{raw:?}");
        }
    }
}

#[test]
fn scripts_print_what_they_print_under_std() {
    let dir = TempDir::new("batch-run");
    let bat = script(&dir, "show.bat");
    // Arguments that wine's `cmd.exe` handles as Windows' does.
    let args = [
        "plain",
        "two words",
        "",
        "q\"uote",
        "%PATH%",
        "%%",
        "&echo INJECTED",
        "a|b",
        "trail\\",
        "\u{e9}",
    ];
    for arg in args {
        let a = Command::new(lite(&bat)).arg(arg).output().unwrap();
        let b = std::process::Command::new(&bat).arg(arg).output().unwrap();
        assert_eq!(a.stdout, b.stdout, "{arg:?}");
        assert_eq!(a.stderr, b.stderr, "{arg:?}");
        assert_eq!(a.status.code(), b.status.code(), "{arg:?}");
        let text = String::from_utf8_lossy(&a.stdout);
        assert!(text.starts_with("ARGS="), "{arg:?}: {text}");
        // `cmd.exe` expanded no variable, and ran no second command.
        assert!(!text.contains(";C:\\"), "{arg:?}: {text}");
        assert!(!text.lines().any(|l| l.trim() == "INJECTED"), "{text}");
    }
}
