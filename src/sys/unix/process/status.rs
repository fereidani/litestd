//! Waiting for a child, and its wait status.

use core::{ffi::c_int, fmt};

use crate::{
    io,
    sys::os::{cvt, cvt_r},
};

/// A spawned child, and its status once reaped.
pub(crate) struct Process {
    pid: libc::pid_t,
    status: Option<ExitStatus>,
}

impl Process {
    pub(crate) const fn new(pid: libc::pid_t) -> Self {
        Self { pid, status: None }
    }

    pub(crate) fn id(&self) -> u32 {
        // Process ids are positive, so the conversion never falls back.
        u32::try_from(self.pid).unwrap_or(0)
    }

    /// Sends `SIGKILL`, unless the child was reaped: its id may then belong
    /// to another process, so this returns `Ok` without a signal, as std
    /// does.
    pub(crate) fn kill(&mut self) -> io::Result<()> {
        if self.status.is_some() {
            return Ok(());
        }
        // SAFETY: `kill` touches no memory.
        cvt(unsafe { libc::kill(self.pid, libc::SIGKILL) }).map(drop)
    }

    pub(crate) fn wait(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.status {
            return Ok(status);
        }
        let mut raw = 0;
        // SAFETY: `raw` is valid for writes of the status.
        cvt_r(|| unsafe { libc::waitpid(self.pid, &raw mut raw, 0) })?;
        let status = ExitStatus(raw);
        self.status = Some(status);
        Ok(status)
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if let Some(status) = self.status {
            return Ok(Some(status));
        }
        let mut raw = 0;
        // SAFETY: `raw` is valid for writes of the status.
        let pid = cvt(unsafe {
            libc::waitpid(self.pid, &raw mut raw, libc::WNOHANG)
        })?;
        if pid == 0 {
            return Ok(None);
        }
        let status = ExitStatus(raw);
        self.status = Some(status);
        Ok(Some(status))
    }
}

/// A wait status, which on Unix describes every way a process can end, not
/// only the code it passed to `exit`.
#[derive(PartialEq, Eq, Clone, Copy, Default)]
pub(crate) struct ExitStatus(c_int);

impl ExitStatus {
    pub(crate) const fn from_raw(raw: c_int) -> Self {
        Self(raw)
    }

    pub(crate) const fn into_raw(self) -> c_int {
        self.0
    }

    /// Only a zero status is an exit with code 0, on every Unix.
    pub(crate) const fn success(self) -> bool {
        self.0 == 0
    }

    pub(crate) const fn code(self) -> Option<i32> {
        if libc::WIFEXITED(self.0) {
            Some(libc::WEXITSTATUS(self.0))
        } else {
            None
        }
    }

    pub(crate) const fn signal(self) -> Option<i32> {
        if libc::WIFSIGNALED(self.0) {
            Some(libc::WTERMSIG(self.0))
        } else {
            None
        }
    }

    pub(crate) const fn core_dumped(self) -> bool {
        libc::WIFSIGNALED(self.0) && libc::WCOREDUMP(self.0)
    }

    pub(crate) const fn stopped_signal(self) -> Option<i32> {
        if libc::WIFSTOPPED(self.0) {
            Some(libc::WSTOPSIG(self.0))
        } else {
            None
        }
    }

    pub(crate) const fn continued(self) -> bool {
        libc::WIFCONTINUED(self.0)
    }
}

impl fmt::Debug for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("unix_wait_status").field(&self.0).finish()
    }
}

impl fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(code) = self.code() {
            write!(f, "exit status: {code}")
        } else if let Some(signal) = self.signal() {
            let name = signal_name(signal);
            let dumped = if self.core_dumped() {
                " (core dumped)"
            } else {
                ""
            };
            write!(f, "signal: {signal}{name}{dumped}")
        } else if let Some(signal) = self.stopped_signal() {
            let name = signal_name(signal);
            write!(f, "stopped (not terminated) by signal: {signal}{name}")
        } else if self.continued() {
            f.write_str("continued (WIFCONTINUED)")
        } else {
            write!(f, "unrecognised wait status: {} {:#x}", self.0, self.0)
        }
    }
}

/// The name std shows after a signal number, as ` (SIGKILL)`, or nothing
/// for a signal it does not name.
const fn signal_name(signal: c_int) -> &'static str {
    /// Matches `signal` against the `libc` constants named, in order.
    macro_rules! names {
        ($($(#[$attr:meta])* $name:ident)*) => {
            match signal {
                $($(#[$attr])* libc::$name => {
                    concat!(" (", stringify!($name), ")")
                })*
                _ => "",
            }
        };
    }
    names!(
        SIGHUP SIGINT SIGQUIT SIGILL SIGTRAP SIGABRT SIGBUS SIGFPE SIGKILL
        SIGUSR1 SIGSEGV SIGUSR2 SIGPIPE SIGALRM SIGTERM SIGCHLD SIGCONT
        SIGSTOP SIGTSTP SIGTTIN SIGTTOU SIGURG SIGXCPU SIGXFSZ SIGVTALRM
        SIGPROF SIGWINCH SIGIO SIGSYS
        #[cfg(all(
            target_os = "linux",
            any(
                target_arch = "x86_64",
                target_arch = "x86",
                target_arch = "arm",
                target_arch = "aarch64"
            )
        ))]
        SIGSTKFLT
        #[cfg(target_os = "linux")]
        SIGPWR
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        SIGEMT
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        SIGINFO
        #[cfg(target_os = "freebsd")]
        SIGTHR
        #[cfg(target_os = "freebsd")]
        SIGLIBRT
    )
}
