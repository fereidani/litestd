//! Spawning with `fork` and `exec`, for what `posix_spawn` cannot do: user
//! and group changes, `pre_exec` closures, and searching a changed `PATH`.
//!
//! After `fork`, only async-signal-safe calls are allowed, since another
//! thread may have held a lock at the time: the child must not allocate or
//! free. Everything it uses is built before forking, and it ends in `execve`
//! or `_exit`, never returning into the caller.

use core::{
    ffi::{c_char, c_int},
    mem, ptr,
};

use super::{Ids, PreExec, environ::Candidates};
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use crate::sys::os::errno;
use crate::{
    io,
    sys::{
        os::{cvt, cvt_r},
        pipe,
    },
    unwind::abort_on_unwind,
};

/// Marks the report of a failed `exec` on the pipe, after the error code.
const EXEC_FAILED: [u8; 4] = *b"NOEX";

/// Everything the child needs, built by the parent.
pub(super) struct Child<'a> {
    /// The descriptors for the standard streams; `-1` inherits.
    pub(super) stdio: [c_int; 3],
    pub(super) ids: Ids,
    pub(super) cwd: Option<*const c_char>,
    pub(super) closures: &'a mut [PreExec],
    /// The arguments, ended by null.
    pub(super) argv: &'a [*const c_char],
    pub(super) envp: *const *const c_char,
    /// The C library's `environ`, found by the parent as `dlsym` is not
    /// async-signal-safe; null if the program has none that litestd finds.
    #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
    pub(super) environ: *mut *const *const c_char,
    pub(super) candidates: &'a Candidates,
    /// `argv` for running a file without a `#!` line with `/bin/sh`, as the
    /// `execvp` of glibc and macOS does: the shell's name for itself, a slot
    /// for the file, and the arguments after the program's name.
    #[cfg(any(target_env = "gnu", target_vendor = "apple"))]
    pub(super) script_argv: alloc_crate::vec::Vec<*const c_char>,
}

/// Forks and execs, returning the child's id once the program runs. An
/// `exec` failure, or one of the steps before it, is reported back through
/// a close-on-exec pipe, which the successful `exec` closes unwritten.
pub(super) fn spawn(child: &mut Child<'_>) -> io::Result<libc::pid_t> {
    let (reader, writer) = pipe::pipe()?;
    // SAFETY: the child only makes async-signal-safe calls, apart from the
    // `pre_exec` closures, whose callers promise the same, and never returns.
    let pid = cvt(unsafe { libc::fork() })?;
    if pid == 0 {
        run_child(child, writer.raw());
    }
    drop(writer);
    let mut report = [0u8; 8];
    // Retries only after a signal interrupted the read.
    let result = loop {
        match reader.read(&mut report) {
            Ok(0) => return Ok(pid),
            Ok(8) if report.ends_with(&EXEC_FAILED) => {
                let code = report.first_chunk().copied().unwrap_or_default();
                break io::Error::from_raw_os_error(i32::from_be_bytes(code));
            }
            // The child writes its report at once and pipes deliver up to
            // `PIPE_BUF` bytes whole, so this cannot happen.
            Ok(_) => {
                break io::const_error!(
                    io::ErrorKind::Other,
                    "short read on the exec status pipe",
                );
            }
            Err(e) if e.is_interrupted() => {}
            Err(e) => break e,
        }
    };
    reap(pid);
    Err(result)
}

/// Collects a child that failed to start, which exits right after its
/// report.
fn reap(pid: libc::pid_t) {
    let mut status = 0;
    // Fails only if the child was reaped elsewhere, as when the program
    // ignores `SIGCHLD`; it is gone either way, which is all that matters.
    // SAFETY: `status` is valid for writes of the status.
    let _ = cvt_r(|| unsafe { libc::waitpid(pid, &raw mut status, 0) });
}

/// The child's side of `spawn`: execs, or reports why not and exits.
fn run_child(child: &mut Child<'_>, report: c_int) -> ! {
    let err = exec(child);
    let code = err.raw_os_error().unwrap_or(libc::EINVAL);
    // A closure's error may own memory, which the child must not free.
    mem::forget(err);
    let mut message = [0u8; 8];
    let (head, tail) = message.split_at_mut(4);
    head.copy_from_slice(&code.to_be_bytes());
    tail.copy_from_slice(&EXEC_FAILED);
    // SAFETY: `message` is valid for reads of its 8 bytes. Nothing can be
    // done if the write fails; the parent then sees the pipe close.
    unsafe { libc::write(report, message.as_ptr().cast(), message.len()) };
    // SAFETY: `_exit` ends the process without running exit handlers, which
    // belong to the parent.
    unsafe { libc::_exit(1) }
}

/// Sets this process up as `child` describes and execs the program,
/// returning only on failure. `Command::exec` calls it without forking.
pub(super) fn exec(child: &mut Child<'_>) -> io::Error {
    match setup(child) {
        Ok(()) => search(child),
        Err(e) => e,
    }
}

/// Applies the settings in std's order: streams, groups and user, working
/// directory, process group, `SIGPIPE`, then the `pre_exec` closures.
fn setup(child: &mut Child<'_>) -> io::Result<()> {
    let streams =
        [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO];
    for (target, fd) in streams.into_iter().zip(child.stdio) {
        if fd >= 0 {
            // SAFETY: `dup2` touches no memory.
            cvt_r(|| unsafe { libc::dup2(fd, target) })?;
        }
    }
    if let Some(gid) = child.ids.gid {
        // SAFETY: `setgid` touches no memory.
        cvt(unsafe { libc::setgid(gid) })?;
    }
    if let Some(uid) = child.ids.uid {
        // Dropping the supplementary groups keeps them from granting what the
        // new user may not have; only a process allowed to change groups
        // can, so `EPERM` is ignored, as in std.
        // SAFETY: an empty list is never read.
        if let Err(e) = cvt(unsafe { libc::setgroups(0, ptr::null()) }) {
            if e.raw_os_error() != Some(libc::EPERM) {
                return Err(e);
            }
        }
        // SAFETY: `setuid` touches no memory.
        cvt(unsafe { libc::setuid(uid) })?;
    }
    if let Some(cwd) = child.cwd {
        // SAFETY: `cwd` is a C string that the parent keeps alive.
        cvt(unsafe { libc::chdir(cwd) })?;
    }
    if let Some(pgroup) = child.ids.pgroup {
        // SAFETY: `setpgid` touches no memory.
        cvt(unsafe { libc::setpgid(0, pgroup) })?;
    }
    default_sigpipe()?;
    for closure in child.closures.iter_mut() {
        // An unwind would return into a copy of the parent's code.
        abort_on_unwind(closure)?;
    }
    Ok(())
}

/// Resets `SIGPIPE` to its default action, which an ignored signal would
/// otherwise keep across `exec`, as std does for compatibility.
fn default_sigpipe() -> io::Result<()> {
    // SAFETY: an all-zero `sigaction` is valid: no flags and an empty mask.
    let mut action: libc::sigaction = unsafe { mem::zeroed() };
    action.sa_sigaction = libc::SIG_DFL;
    // SAFETY: `action` is valid for reads; the old action is not wanted.
    cvt(unsafe {
        libc::sigaction(libc::SIGPIPE, &raw const action, ptr::null_mut())
    })
    .map(drop)
}

/// Execs the program as std's child does on the BSDs and Android: with the
/// C library's `execvp`, whose rules differ between them, after making the
/// child's environment the process's, whose `PATH` it searches.
/// `Command::exec` runs this without forking, so the environment is
/// restored on failure.
#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
fn search(child: &mut Child<'_>) -> io::Error {
    let slot = child.environ;
    if slot.is_null() {
        // Without `environ`, the parent read the environment as empty, so an
        // inherited one is null: the child keeps the process's, but cannot
        // get another.
        if !child.envp.is_null() {
            return io::const_error!(
                io::ErrorKind::Unsupported,
                "cannot set the environment without `environ`",
            );
        }
        return execvp(child);
    }
    // SAFETY: `slot` is the C library's `environ`, found by the parent. A
    // forked child runs no other thread, and `Command::exec` holds the
    // environment lock, as std's does; `envp` outlives the call.
    let old = unsafe { slot.replace(child.envp) };
    let err = execvp(child);
    // SAFETY: as above; `old` was the environment before.
    unsafe { slot.write(old) };
    err
}

/// Calls the C library's `execvp` on the program, returning its error.
#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
fn execvp(child: &Child<'_>) -> io::Error {
    // SAFETY: `paths` holds the program as a C string, and `argv` is a
    // null-terminated array of C strings, all kept alive by the parent.
    unsafe {
        libc::execvp(
            child.candidates.paths.as_ptr().cast(),
            child.argv.as_ptr(),
        )
    };
    io::Error::last_os_error()
}

/// Execs the first candidate that works, as the `execvp` of glibc and musl
/// does, returning the error it would: the first one that means an
/// executable was found but could not run, else `EACCES` if some candidate
/// was not accessible, else the last error.
#[cfg(target_os = "linux")]
fn search(child: &mut Child<'_>) -> io::Error {
    let mut denied = false;
    let mut err = child.candidates.err;
    let mut rest: &[u8] = &child.candidates.paths;
    // Each pass consumes one NUL-terminated path, so the loop ends.
    while let Some(end) = rest.iter().position(|&b| b == 0) {
        let path = rest.as_ptr().cast::<c_char>();
        // SAFETY: `path` is a C string, and `argv` and `envp` are
        // null-terminated arrays of C strings, all kept alive by the parent.
        unsafe { libc::execve(path, child.argv.as_ptr(), child.envp) };
        err = errno();
        #[cfg(target_env = "gnu")]
        if err == libc::ENOEXEC {
            err = exec_script(child, path);
        }
        match err {
            libc::EACCES => denied = true,
            libc::ENOENT | libc::ENOTDIR => {}
            #[cfg(target_env = "gnu")]
            libc::ESTALE | libc::ENODEV | libc::ETIMEDOUT => {}
            _ => return io::Error::from_raw_os_error(err),
        }
        rest = rest.get(end + 1..).unwrap_or_default();
    }
    io::Error::from_raw_os_error(if denied { libc::EACCES } else { err })
}

/// Execs the first candidate that works, as macOS's `execvp` does,
/// returning the error it would. Errors that leave a doubt, `EACCES` first,
/// are settled with `stat`: a candidate that exists but cannot run makes
/// `EACCES` the result unless a later one runs, and any other error of an
/// existing file ends the search. A search that finds nothing reports
/// `ENOENT`, and a single path its own error.
#[cfg(target_vendor = "apple")]
fn search(child: &mut Child<'_>) -> io::Error {
    let mut denied = false;
    let mut err = child.candidates.err;
    let mut rest: &[u8] = &child.candidates.paths;
    // Each pass consumes one NUL-terminated path, so the loop ends.
    while let Some(end) = rest.iter().position(|&b| b == 0) {
        let path = rest.as_ptr().cast::<c_char>();
        err = execve(child, path);
        match err {
            libc::ELOOP | libc::ENAMETOOLONG | libc::ENOENT | libc::ENOTDIR => {
            }
            libc::E2BIG | libc::ENOMEM | libc::ETXTBSY => {
                return io::Error::from_raw_os_error(err);
            }
            libc::ENOEXEC => {
                return io::Error::from_raw_os_error(exec_script(child, path));
            }
            _ => match stat_error(path) {
                Some(stat_err) => err = stat_err,
                None if err == libc::EACCES => denied = true,
                None => return io::Error::from_raw_os_error(err),
            },
        }
        rest = rest.get(end + 1..).unwrap_or_default();
    }
    let code = match (denied, child.candidates.search) {
        (true, _) => libc::EACCES,
        (false, true) => libc::ENOENT,
        (false, false) => err,
    };
    io::Error::from_raw_os_error(code)
}

/// Returns the error of `stat` on `path`, or `None` if the file exists.
#[cfg(target_vendor = "apple")]
fn stat_error(path: *const c_char) -> Option<c_int> {
    let mut st = mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `path` is a C string and `st` valid for writes of a `stat`;
    // `stat` is async-signal-safe.
    let r = unsafe { libc::stat(path, st.as_mut_ptr()) };
    (r != 0).then(errno)
}

/// Execs `path` with the child's arguments and environment, returning the
/// error if that fails.
#[cfg(target_vendor = "apple")]
fn execve(child: &Child<'_>, path: *const c_char) -> c_int {
    // SAFETY: `path` is a C string, and `argv` and `envp` are
    // null-terminated arrays of C strings, all kept alive by the parent.
    unsafe { libc::execve(path, child.argv.as_ptr(), child.envp) };
    errno()
}

/// Runs `path`, which has no `#!` line, with `/bin/sh`, as the `execvp` of
/// glibc and macOS does, returning the error if that fails.
#[cfg(any(target_env = "gnu", target_vendor = "apple"))]
fn exec_script(child: &mut Child<'_>, path: *const c_char) -> c_int {
    if let Some(slot) = child.script_argv.get_mut(1) {
        *slot = path;
    }
    // SAFETY: as for the `execve` of `search`; `script_argv` holds C strings
    // and ends with null.
    unsafe {
        libc::execve(
            c"/bin/sh".as_ptr(),
            child.script_argv.as_ptr(),
            child.envp,
        )
    };
    errno()
}

/// Builds the `argv` of `exec_script` from the command's `argv`: the shell
/// is named `/bin/sh` by glibc, and after the program by macOS.
#[cfg(any(target_env = "gnu", target_vendor = "apple"))]
pub(super) fn script_argv(
    argv: &[*const c_char],
) -> alloc_crate::vec::Vec<*const c_char> {
    let mut script = alloc_crate::vec::Vec::with_capacity(argv.len() + 1);
    #[cfg(target_env = "gnu")]
    script.push(c"/bin/sh".as_ptr());
    #[cfg(target_vendor = "apple")]
    script.push(argv.first().copied().unwrap_or(c"sh".as_ptr()));
    script.push(ptr::null());
    // The arguments after the program's name, and the terminator.
    script.extend_from_slice(argv.get(1..).unwrap_or(&[ptr::null()]));
    script
}
