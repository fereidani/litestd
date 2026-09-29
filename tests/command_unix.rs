//! Unix specifics of `litestd::process` against std: signals, `CommandExt`,
//! `ExitStatusExt`, the `PATH` search, descriptor hygiene and both spawn
//! paths (`posix_spawn`, and `fork` when a `pre_exec` closure is set).
//!
//! Every child runs in a scratch directory of its own, unless the test is
//! about the working directory. Miri cannot spawn processes.

#![cfg(all(unix, feature = "command", not(miri)))]
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    reason = "a failure fails the test"
)]

use core::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};
use std::{
    io::Write as _,
    os::unix::process::{CommandExt as _, ExitStatusExt as _},
    path::PathBuf,
};

use litestd::{
    io::{Read as _, Write as _},
    os::unix::process::{CommandExt, ExitStatusExt},
    process as lite,
};

/// The environment variable that turns this binary into a helper child.
const MODE: &str = "LITESTD_COMMAND_UNIX_CHILD";

/// `true` and `false`, in `/bin` on Linux and in `/usr/bin` on macOS and
/// the BSDs.
const TRUE: &str = if cfg!(not(any(target_os = "linux", target_os = "android")))
{
    "/usr/bin/true"
} else {
    "/bin/true"
};
const FALSE: &str =
    if cfg!(not(any(target_os = "linux", target_os = "android"))) {
        "/usr/bin/false"
    } else {
        "/bin/false"
    };

/// Where a process finds its open descriptors listed.
const FD_DIR: &str =
    if cfg!(not(any(target_os = "linux", target_os = "android"))) {
        "/dev/fd"
    } else {
        "/proc/self/fd"
    };

/// A directory of its own for one test, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        let dir = format!("litestd-unix-{name}-{}-{n}", std::process::id());
        let path = std::env::temp_dir().join(dir);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }

    /// Creates `name` in the directory with `text` and permission `mode`.
    fn file(&self, name: &str, text: &str, mode: u32) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = self.0.join(name);
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
            .unwrap();
        path.to_str().unwrap().to_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Builds the same command for litestd and std, running in `dir`.
fn pair(
    program: &str,
    args: &[&str],
    dir: &Scratch,
) -> (lite::Command, std::process::Command) {
    let mut lite = lite::Command::new(program);
    lite.args(args).current_dir(dir.path());
    let mut std = std::process::Command::new(program);
    std.args(args).current_dir(&dir.0);
    (lite, std)
}

/// The outcomes of `output` for both, which are run again while either
/// fails with `ETXTBSY`: a child that another test forks holds a copy of
/// every descriptor until it execs, that of a script just written included,
/// and the kernel refuses to run a file open for writing.
fn outputs(
    lite: &mut lite::Command,
    std: &mut std::process::Command,
) -> (
    litestd::io::Result<lite::Output>,
    std::io::Result<std::process::Output>,
) {
    // At most ten seconds, far longer than any fork takes to exec.
    for _ in 0..1000 {
        let (l, s) = (lite.output(), std.output());
        let busy = l.as_ref().err().and_then(litestd::io::Error::raw_os_error)
            == Some(libc::ETXTBSY)
            || s.as_ref().err().and_then(std::io::Error::raw_os_error)
                == Some(libc::ETXTBSY);
        if !busy {
            return (l, s);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the program stayed busy");
}

/// Runs both with `output` and checks that they agree, raw status included.
fn same_output(
    lite: &mut lite::Command,
    std: &mut std::process::Command,
) -> std::process::Output {
    let (l, s) = outputs(lite, std);
    let (l, s) = (l.unwrap(), s.unwrap());
    same_run(&l, &s);
    s
}

/// Checks that two runs agree: status, raw and shown, and output.
fn same_run(l: &lite::Output, s: &std::process::Output) {
    assert_eq!(l.status.into_raw(), s.status.into_raw(), "{s:?}");
    assert_eq!(l.status.to_string(), s.status.to_string());
    assert_eq!(l.stdout, s.stdout, "stdout of {s:?}");
    assert_eq!(l.stderr, s.stderr, "stderr of {s:?}");
}

/// Runs both with `output`: both run and agree as in `same_output`, giving
/// std's stdout, or both fail to start alike, giving the error number.
fn same_outcome(
    lite: &mut lite::Command,
    std: &mut std::process::Command,
) -> Result<Vec<u8>, Option<i32>> {
    match outputs(lite, std) {
        (Ok(l), Ok(s)) => {
            same_run(&l, &s);
            Ok(s.stdout)
        }
        (Err(l), Err(s)) => {
            assert_eq!(l.raw_os_error(), s.raw_os_error(), "{s}");
            assert_eq!(l.to_string(), s.to_string());
            Err(s.raw_os_error())
        }
        (l, s) => panic!("litestd {l:?}, std {s:?}"),
    }
}

/// A `pre_exec` closure that does nothing, to take the `fork` path.
fn force_fork(cmd: &mut lite::Command) -> &mut lite::Command {
    // SAFETY: the closure makes no calls at all.
    unsafe { cmd.pre_exec(|| Ok(())) }
}

/// The helper side of the tests; a no-op unless `MODE` is set.
#[test]
fn child() {
    let Ok(mode) = std::env::var(MODE) else {
        return;
    };
    let script = ["-c", "echo exec-ok \"$0\""];
    match mode.as_str() {
        "exec" => {
            let mut cmd = lite::Command::new("/bin/sh");
            let err = cmd.args(script).arg0("renamed").exec();
            println!("exec returned {err}");
        }
        "exec-std" => {
            let mut cmd = std::process::Command::new("/bin/sh");
            let err = cmd.args(script).arg0("renamed").exec();
            println!("exec returned {err}");
        }
        "leaks" => println!("{}", leaks_no_fds()),
        "fds" => standard_streams_only(),
        #[cfg(feature = "env")]
        "env-race" => println!("{}", env_race()),
        "exec-missing" => {
            let lite = lite::Command::new("/no/such/program").exec();
            let std = std::process::Command::new("/no/such/program").exec();
            println!("{:?} {:?}", lite.raw_os_error(), std.raw_os_error());
            let lite = lite::Command::new("litestd-no-such").exec();
            let std = std::process::Command::new("litestd-no-such").exec();
            println!("{:?} {:?}", lite.raw_os_error(), std.raw_os_error());
        }
        other => panic!("unknown mode {other}"),
    }
    std::io::stdout().flush().unwrap();
    std::process::exit(0);
}

/// Runs this binary as a helper in `mode` with std, returning its stdout.
fn run_helper(mode: &str, dir: &Scratch) -> String {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child", "--nocapture", "--test-threads=1", "-q"])
        .env(MODE, mode)
        .current_dir(&dir.0)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn shell_statuses_and_signals_match_std() {
    let dir = Scratch::new("signals");
    for script in [
        "exit 0",
        "exit 42",
        "kill -9 $$",
        "kill -TERM $$",
        "echo out",
    ] {
        let (mut lite, mut std) = pair("/bin/sh", &["-c", script], &dir);
        let out = same_output(&mut lite, &mut std);
        let status = lite.status().unwrap();
        assert_eq!(status.signal(), out.status.signal());
        assert_eq!(status.core_dumped(), out.status.core_dumped());
        assert_eq!(status.stopped_signal(), None);
        assert!(!status.continued());
    }
}

#[test]
fn true_false_and_cat_match_std() {
    let dir = Scratch::new("coreutils");
    for program in [TRUE, FALSE, "true", "false"] {
        let (mut lite, mut std) = pair(program, &[], &dir);
        same_output(&mut lite, &mut std);
        force_fork(&mut lite);
        same_output(&mut lite, &mut std);
    }
    let (mut lite, _) = pair("/bin/cat", &[], &dir);
    let mut child = lite
        .stdin(lite::Stdio::piped())
        .stdout(lite::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.as_mut().unwrap().write_all(b"meow").unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.stdout, b"meow");
}

#[test]
fn arg0_names_the_child() {
    let dir = Scratch::new("arg0");
    let (mut lite, mut std) = pair("/bin/sh", &["-c", "echo \"$0\""], &dir);
    lite.arg0("custom name");
    std.arg0("custom name");
    let out = same_output(&mut lite, &mut std);
    assert_eq!(out.stdout, b"custom name\n");
    force_fork(&mut lite);
    same_output(&mut lite, &mut std);
}

/// A command that prints its own process id and process group id, which
/// `pid_and_pgrp` reads: its `/proc/self/stat` line on Linux, and what `ps`
/// reports for the shell elsewhere.
#[cfg(any(target_os = "linux", target_os = "android"))]
const IDS: (&str, &[&str]) = ("/bin/cat", &["/proc/self/stat"]);
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const IDS: (&str, &[&str]) =
    ("/bin/sh", &["-c", "echo $$ $(ps -o pgid= -p $$)"]);

/// The process id and process group id in the output of `IDS`.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn pid_and_pgrp(stat: &[u8]) -> (String, String) {
    let text = String::from_utf8(stat.to_vec()).unwrap();
    // The command name, in parentheses, may hold spaces; fields follow it.
    let (pid, rest) = text.split_once(" (").unwrap();
    let fields: Vec<&str> =
        rest.rsplit_once(") ").unwrap().1.split(' ').collect();
    (pid.to_owned(), fields[2].to_owned())
}

/// The process id and process group id in the output of `IDS`.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn pid_and_pgrp(out: &[u8]) -> (String, String) {
    let text = String::from_utf8(out.to_vec()).unwrap();
    let mut ids = text.split_whitespace();
    let (pid, pgrp) = (ids.next().unwrap(), ids.next().unwrap());
    (pid.to_owned(), pgrp.to_owned())
}

#[test]
fn process_group_matches_std() {
    let dir = Scratch::new("pgroup");
    let (mut lite, mut std) = pair(IDS.0, IDS.1, &dir);
    // SAFETY: `getpgrp` has no preconditions.
    let own = unsafe { libc::getpgrp() }.to_string();
    assert_eq!(pid_and_pgrp(&lite.output().unwrap().stdout).1, own);
    lite.process_group(0);
    std.process_group(0);
    let (pid, pgrp) = pid_and_pgrp(&std.output().unwrap().stdout);
    assert_eq!(pid, pgrp);
    let (pid, pgrp) = pid_and_pgrp(&lite.output().unwrap().stdout);
    assert_eq!(pid, pgrp);
    force_fork(&mut lite);
    let (pid, pgrp) = pid_and_pgrp(&lite.output().unwrap().stdout);
    assert_eq!(pid, pgrp);
}

#[test]
fn exec_replaces_the_process_like_std() {
    let dir = Scratch::new("exec");
    let lite = run_helper("exec", &dir);
    let std = run_helper("exec-std", &dir);
    assert_eq!(lite, std);
    assert!(lite.ends_with("exec-ok renamed\n"), "{lite}");
}

#[test]
fn exec_failures_match_std() {
    let dir = Scratch::new("execfail");
    let out = run_helper("exec-missing", &dir);
    let lines: Vec<&str> = out.lines().rev().take(2).collect();
    for line in lines {
        let (lite, std) = line.split_once(' ').unwrap();
        assert_eq!(lite, std, "{out}");
        assert_eq!(lite, format!("Some({})", libc::ENOENT));
    }
}

/// Creates a pipe with the C library, close-on-exec: `(reader, writer)`.
fn raw_pipe() -> (std::fs::File, std::fs::File) {
    use std::os::fd::FromRawFd;
    let mut fds = [0; 2];
    // SAFETY: `fds` is valid for writes of two descriptors.
    #[cfg(not(target_vendor = "apple"))]
    assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
    // macOS has no `pipe2`.
    #[cfg(target_vendor = "apple")]
    {
        // SAFETY: as above.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        for fd in fds {
            // SAFETY: `F_SETFD` touches no memory.
            let r = unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
            assert_eq!(r, 0);
        }
    }
    // SAFETY: the call returned two new descriptors that nothing owns.
    unsafe {
        (
            std::fs::File::from_raw_fd(fds[0]),
            std::fs::File::from_raw_fd(fds[1]),
        )
    }
}

#[test]
fn pre_exec_closures_run_in_order_until_one_fails() {
    use std::os::fd::AsRawFd;
    let dir = Scratch::new("preexec");
    let (mut reader, writer) = raw_pipe();
    let fd = writer.as_raw_fd();
    let mark = move |byte: u8| {
        move || {
            // SAFETY: `write` is async-signal-safe, and `fd` stays open in
            // the child, which inherited it.
            unsafe { libc::write(fd, (&raw const byte).cast(), 1) };
            Ok(())
        }
    };
    let (mut lite, _) = pair(TRUE, &[], &dir);
    // SAFETY: the closures only call `write`.
    unsafe {
        lite.pre_exec(mark(b'1'));
        lite.pre_exec(|| {
            Err(litestd::io::Error::from_raw_os_error(libc::EPERM))
        });
        lite.pre_exec(mark(b'3'));
    }
    let err = lite.spawn().unwrap_err();
    assert_eq!(err.raw_os_error(), Some(libc::EPERM));
    drop(writer);
    let mut seen = String::new();
    std::io::Read::read_to_string(&mut reader, &mut seen).unwrap();
    assert_eq!(seen, "1");
}

#[test]
fn user_and_group_changes_match_std() {
    let dir = Scratch::new("ids");
    // Root switches to `nobody`, and others try to switch to root, which
    // fails, as it does for a root without `CAP_SETUID`, as in some
    // containers: both run alike or fail alike.
    // SAFETY: `geteuid` has no preconditions.
    let target = if unsafe { libc::geteuid() } == 0 {
        65534
    } else {
        0
    };
    let (mut lite, mut std) = pair("/bin/sh", &["-c", "id -u; id -g"], &dir);
    lite.uid(target).gid(target);
    std.uid(target).gid(target);
    if let Ok(stdout) = same_outcome(&mut lite, &mut std) {
        assert_eq!(stdout, format!("{target}\n{target}\n").into_bytes());
    }
}

#[test]
fn a_changed_path_is_searched_like_std() {
    let dir = Scratch::new("path");
    dir.file("litestd-probe", "#!/bin/sh\necho found \"$0\"\n", 0o755);
    dir.file("litestd-plain", "echo plain\n", 0o755);
    dir.file("litestd-locked", "#!/bin/sh\n", 0o644);
    let search = format!("/litestd/missing:{}:/usr/bin:/bin", dir.path());
    for program in ["litestd-probe", "litestd-plain", "sh"] {
        let (mut lite, mut std) = pair(program, &["-c", "echo sh"], &dir);
        lite.env("PATH", &search);
        std.env("PATH", &search);
        let outcome = same_outcome(&mut lite, &mut std);
        if program == "litestd-plain" {
            assert_eq!(outcome, plain_script_outcome(b"plain\n"));
        } else {
            assert!(outcome.is_ok(), "{program}: {outcome:?}");
        }
    }
    let (mut lite, mut std) = pair("litestd-locked", &[], &dir);
    lite.env("PATH", &search);
    std.env("PATH", &search);
    let (l, s) = (lite.spawn().unwrap_err(), std.spawn().unwrap_err());
    assert_eq!(l.raw_os_error(), s.raw_os_error());
    assert_eq!(l.raw_os_error(), Some(libc::EACCES));
    // Without `PATH`, the C library's default list is searched.
    let (mut lite, mut std) = pair("sh", &["-c", "echo default"], &dir);
    lite.env_clear();
    std.env_clear();
    assert_eq!(same_output(&mut lite, &mut std).stdout, b"default\n");
}

#[test]
fn relative_programs_with_a_working_directory_match_std() {
    let dir = Scratch::new("relative");
    dir.file("probe", "#!/bin/sh\necho relative\n", 0o755);
    let (mut lite, mut std) = pair("./probe", &[], &dir);
    assert_eq!(same_output(&mut lite, &mut std).stdout, b"relative\n");
    force_fork(&mut lite);
    same_output(&mut lite, &mut std);
}

/// What the `fork` path gives for an executable without a `#!` line that
/// prints `stdout` when run with `/bin/sh`: every C library's `execvp`
/// retries it that way, except musl's, which fails with `ENOEXEC`.
fn plain_script_outcome(stdout: &[u8]) -> Result<Vec<u8>, Option<i32>> {
    if cfg!(target_env = "musl") {
        Err(Some(libc::ENOEXEC))
    } else {
        Ok(stdout.to_vec())
    }
}

/// A script without a `#!` line fails with `ENOEXEC` through the
/// `posix_spawn` of glibc and musl, which std uses unless glibc is too old
/// for it, while macOS's runs it with `/bin/sh`; through `fork` it is as the
/// C library's `execvp` decides. litestd follows std on both paths.
#[test]
fn scripts_without_an_interpreter_line_match_std() {
    let dir = Scratch::new("noshebang");
    dir.file("plain", "echo plain \"$@\"\n", 0o755);
    let (mut lite, mut std) = pair("./plain", &["arg"], &dir);
    let spawned = same_outcome(&mut lite, &mut std);
    if cfg!(target_env = "musl") {
        assert_eq!(spawned, Err(Some(libc::ENOEXEC)));
    }
    force_fork(&mut lite);
    // SAFETY: the closure makes no calls at all.
    unsafe { std.pre_exec(|| Ok(())) };
    let forked = same_outcome(&mut lite, &mut std);
    assert_eq!(forked, plain_script_outcome(b"plain arg\n"));
}

/// Killing a child that exited but was not waited for succeeds, as in std.
#[test]
fn killing_an_exited_child_succeeds() {
    let dir = Scratch::new("zombie");
    let (mut lite, mut std) = pair(TRUE, &[], &dir);
    let (mut l, mut s) = (lite.spawn().unwrap(), std.spawn().unwrap());
    for pid in [l.id(), s.id()] {
        // `WNOWAIT` waits for the child to exit and leaves it a zombie.
        // SAFETY: an all-zero `siginfo_t` is valid, and `waitid` only
        // writes it.
        #[allow(clippy::useless_conversion, reason = "`id_t` varies")]
        let r = unsafe {
            let mut info: libc::siginfo_t = core::mem::zeroed();
            libc::waitid(
                libc::P_PID,
                pid.into(),
                &raw mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        assert_eq!(r, 0);
    }
    l.kill().unwrap();
    s.kill().unwrap();
    assert!(l.wait().unwrap().success());
    assert!(s.wait().unwrap().success());
}

/// Reads the child's signal state from `/proc`, which macOS lacks.
#[cfg(target_os = "linux")]
#[test]
fn signal_state_matches_std() {
    let dir = Scratch::new("sigstate");
    let lines = |out: &[u8]| -> String {
        String::from_utf8_lossy(out)
            .lines()
            .filter(|l| l.starts_with("SigBlk") || l.starts_with("SigIgn"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let (mut lite, mut std) = pair("/bin/cat", &["/proc/self/status"], &dir);
    let std_out = lines(&std.output().unwrap().stdout);
    assert_eq!(lines(&lite.output().unwrap().stdout), std_out);
    // A child inherits the spawning thread's signal mask, as in std.
    // SAFETY: an all-zero `sigset_t` is valid.
    let mut blocked: libc::sigset_t = unsafe { core::mem::zeroed() };
    // SAFETY: the calls only read and write `blocked`, and change the mask
    // of this test thread alone.
    unsafe {
        libc::sigemptyset(&raw mut blocked);
        libc::sigaddset(&raw mut blocked, libc::SIGUSR2);
        libc::pthread_sigmask(
            libc::SIG_BLOCK,
            &raw const blocked,
            core::ptr::null_mut(),
        );
    }
    let std_blocked = lines(&std.output().unwrap().stdout);
    let lite_blocked = lines(&lite.output().unwrap().stdout);
    force_fork(&mut lite);
    let fork_blocked = lines(&lite.output().unwrap().stdout);
    // SAFETY: as above.
    unsafe {
        libc::pthread_sigmask(
            libc::SIG_UNBLOCK,
            &raw const blocked,
            core::ptr::null_mut(),
        );
    }
    assert_ne!(std_blocked, std_out);
    assert_eq!(lite_blocked, std_blocked);
    assert_eq!(fork_blocked, std_blocked);
    assert_eq!(lines(&lite.output().unwrap().stdout), std_out);
}

/// The descriptors open in this process.
fn open_fds() -> usize {
    std::fs::read_dir(FD_DIR).unwrap().count()
}

/// Marks the descriptors this process inherited close-on-exec, so that its
/// children do not get those the test runner leaks, as GitHub's macOS
/// runner does.
fn inherit_no_fds() {
    let fds: Vec<libc::c_int> = std::fs::read_dir(FD_DIR)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_str().unwrap().parse())
        .collect::<Result<_, _>>()
        .unwrap();
    for fd in fds.into_iter().filter(|&fd| fd > 2) {
        // SAFETY: `F_GETFD` and `F_SETFD` touch no memory.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        // The descriptor that listed the directory is closed by now.
        if flags != -1 {
            let set = libc::FD_CLOEXEC | flags;
            // SAFETY: as above.
            assert_eq!(unsafe { libc::fcntl(fd, libc::F_SETFD, set) }, 0);
        }
    }
}

/// Checks that children get only the standard streams: `ls` lists them and
/// the descriptors it opens itself. Run in a helper child, where no other
/// test opens descriptors meanwhile: macOS sets close-on-exec only after it
/// creates a pipe or a socket, as std does there, so a child spawned at that
/// moment would inherit it.
fn standard_streams_only() {
    inherit_no_fds();
    let dir = Scratch::new("fds");
    let (mut lite, mut std) = pair("/bin/ls", &[FD_DIR], &dir);
    lite.stdin(lite::Stdio::piped()).stderr(lite::Stdio::null());
    std.stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let out = same_output(&mut lite, &mut std);
    // `ls` reads the directory through descriptor 3; on macOS, `fts` holds
    // the starting directory open as well, to return to it.
    let own: &[u8] = if cfg!(target_vendor = "apple") {
        b"3\n4\n"
    } else {
        b"3\n"
    };
    if cfg!(any(target_os = "linux", target_vendor = "apple")) {
        assert_eq!(out.stdout, [b"0\n1\n2\n".as_slice(), own].concat());
    }
    force_fork(&mut lite);
    same_output(&mut lite, &mut std);
}

#[test]
fn children_get_only_the_standard_streams() {
    let dir = Scratch::new("fdcheck");
    run_helper("fds", &dir);
}

/// Whether spawning leaves no descriptor open in this process. Run in a
/// helper child, where no other test opens descriptors meanwhile.
fn leaks_no_fds() -> bool {
    let dir = Scratch::new("leaks");
    let (mut lite, _) = pair("/bin/cat", &[], &dir);
    lite.stdin(lite::Stdio::piped())
        .stdout(lite::Stdio::piped());
    lite.stderr(lite::Stdio::piped());
    let _ = lite.output().unwrap();
    let before = open_fds();
    for _ in 0..20 {
        let child = lite.spawn().unwrap();
        child.wait_with_output().unwrap();
        lite.output().unwrap();
        let _ = lite::Command::new("/no/such").spawn().unwrap_err();
    }
    force_fork(&mut lite);
    lite.output().unwrap();
    let _ = force_fork(&mut lite::Command::new("/no/such"))
        .spawn()
        .unwrap_err();
    open_fds() == before
}

#[test]
fn spawning_leaks_no_descriptors() {
    let dir = Scratch::new("leakcheck");
    assert!(run_helper("leaks", &dir).ends_with("\ntrue\n"));
}

/// Children spawned at once from many threads each get only their own
/// pipes: a pipe leaked into another child would keep its reader waiting.
/// Linux only: macOS sets close-on-exec right after it creates a pipe, as
/// std does there, so a child spawned at that moment inherits it.
#[cfg(not(target_vendor = "apple"))]
#[test]
fn concurrent_spawns_do_not_share_pipes() {
    let dir = Scratch::new("concurrent");
    std::thread::scope(|s| {
        for fork in [false, true, false, true] {
            let dir = &dir;
            s.spawn(move || {
                for _ in 0..10 {
                    let (mut lite, _) = pair("/bin/cat", &[], dir);
                    if fork {
                        force_fork(&mut lite);
                    }
                    lite.stdin(lite::Stdio::piped())
                        .stdout(lite::Stdio::piped());
                    let mut child = lite.spawn().unwrap();
                    child.stdin.take().unwrap().write_all(b"x").unwrap();
                    assert_eq!(child.wait_with_output().unwrap().stdout, b"x");
                }
            });
        }
    });
}

#[test]
fn writing_to_a_closed_child_fails_like_std() {
    let dir = Scratch::new("epipe");
    let (mut lite, mut std) = pair(TRUE, &[], &dir);
    let mut l = lite.stdin(lite::Stdio::piped()).spawn().unwrap();
    let mut s = std.stdin(std::process::Stdio::piped()).spawn().unwrap();
    // `try_wait`, unlike `wait`, leaves stdin open.
    while l.try_wait().unwrap().is_none() || s.try_wait().unwrap().is_none() {
        std::thread::sleep(Duration::from_millis(2));
    }
    // The test harness, being std, ignores `SIGPIPE`.
    let big = vec![0u8; 1 << 20];
    let l_err = l.stdin.as_mut().unwrap().write_all(&big).unwrap_err();
    let s_err = s.stdin.as_mut().unwrap().write_all(&big).unwrap_err();
    assert_eq!(l_err.raw_os_error(), s_err.raw_os_error());
    assert_eq!(l_err.raw_os_error(), Some(libc::EPIPE));
}

/// Spawns children, with the environment inherited and with a copy of it
/// changed, while another thread keeps growing and shrinking `environ`
/// through `env::set_var`, which litestd serializes against the spawns.
/// Returns how many times the environment changed meanwhile.
#[cfg(feature = "env")]
fn env_race() -> usize {
    let dir = Scratch::new("envrace");
    let done = AtomicBool::new(false);
    std::thread::scope(|s| {
        let setter = s.spawn(|| {
            let mut rounds = 0;
            while !done.load(Ordering::Relaxed) {
                for i in 0..16 {
                    let value = rounds.to_string();
                    // SAFETY: the other thread reads the environment only
                    // through litestd, which takes the same lock.
                    unsafe {
                        litestd::env::set_var(
                            format!("LITESTD_RACE_{i}"),
                            value,
                        );
                    }
                }
                for i in 0..16 {
                    // SAFETY: as above.
                    unsafe {
                        litestd::env::remove_var(format!("LITESTD_RACE_{i}"));
                    }
                }
                rounds += 1;
            }
            rounds
        });
        let spawns = std::panic::catch_unwind(|| {
            for _ in 0..100 {
                let script = ["-c", "echo ${LITESTD_RACE_15-unset}"];
                let (mut inherit, _) = pair("/bin/sh", &script, &dir);
                let (mut changed, _) = pair("/bin/sh", &script, &dir);
                changed.env("LITESTD_OTHER", "1");
                force_fork(&mut changed);
                for cmd in [&mut inherit, &mut changed] {
                    let out = cmd.output().unwrap();
                    let text = String::from_utf8(out.stdout).unwrap();
                    let text = text.trim_end();
                    assert!(text == "unset" || text.parse::<u32>().is_ok());
                }
            }
        });
        // Stops the setter whatever happened, so a failure cannot hang.
        done.store(true, Ordering::Relaxed);
        let rounds = setter.join().unwrap();
        if let Err(panic) = spawns {
            std::panic::resume_unwind(panic);
        }
        rounds
    })
}

#[cfg(feature = "env")]
#[test]
fn set_var_racing_spawns_is_safe() {
    let dir = Scratch::new("envracecheck");
    let out = run_helper("env-race", &dir);
    let seen: usize = out.lines().last().unwrap().parse().unwrap();
    assert!(seen > 0, "{out}");
}

#[test]
fn parent_id_matches_std() {
    assert_eq!(
        litestd::os::unix::process::parent_id(),
        std::os::unix::process::parent_id()
    );
}

#[cfg(feature = "fs")]
#[test]
fn files_and_descriptors_become_streams() {
    use litestd::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
    let dir = Scratch::new("files");
    let path = dir.0.join("out.txt");
    let file = litestd::fs::File::create(path.to_str().unwrap()).unwrap();
    let (mut lite, _) = pair("/bin/sh", &["-c", "echo to file"], &dir);
    assert!(lite.stdout(file).status().unwrap().success());
    assert_eq!(std::fs::read(&path).unwrap(), b"to file\n");
    let input = litestd::fs::File::open(path.to_str().unwrap()).unwrap();
    let fd = input.into_raw_fd();
    // SAFETY: `fd` is an open descriptor that nothing else owns.
    let stdin = unsafe { lite::Stdio::from_raw_fd(fd) };
    let mut cat = lite::Command::new("/bin/cat");
    cat.current_dir(dir.path()).stdin(stdin);
    assert_eq!(cat.output().unwrap().stdout, b"to file\n");
    let owned =
        OwnedFd::from(litestd::fs::File::open(path.to_str().unwrap()).unwrap());
    assert!(owned.as_raw_fd() > 2);
    let out = cat.stdin(owned).output().unwrap();
    assert_eq!(out.stdout, b"to file\n");
}

#[test]
fn child_pipes_convert_to_and_from_descriptors() {
    use litestd::os::fd::{AsFd, AsRawFd, IntoRawFd, OwnedFd};
    let dir = Scratch::new("pipefd");
    let (mut lite, _) = pair("/bin/cat", &[], &dir);
    let mut child = lite
        .stdin(lite::Stdio::piped())
        .stdout(lite::Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    assert_eq!(stdin.as_fd().as_raw_fd(), stdin.as_raw_fd());
    // SAFETY: `F_GETFD` touches no memory.
    let flags = unsafe { libc::fcntl(stdin.as_raw_fd(), libc::F_GETFD) };
    assert_eq!(flags & libc::FD_CLOEXEC, libc::FD_CLOEXEC);
    let mut stdin = lite::ChildStdin::from(OwnedFd::from(stdin));
    stdin.write_all(b"round trip").unwrap();
    drop(stdin);
    let stdout = child.stdout.take().unwrap();
    let raw = stdout.into_raw_fd();
    // SAFETY: `raw` came from `into_raw_fd`, so this owns it.
    let owned =
        unsafe { <OwnedFd as litestd::os::fd::FromRawFd>::from_raw_fd(raw) };
    let mut stdout = lite::ChildStdout::from(owned);
    let mut text = Vec::new();
    stdout.read_to_end(&mut text).unwrap();
    assert_eq!(text, b"round trip");
    assert!(child.wait().unwrap().success());
}
