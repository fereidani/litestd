//! Unix-specific extensions to primitives in the [`process`] module.

use alloc_crate::boxed::Box;

use crate::{
    ffi::OsStr,
    io,
    os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd},
    process::{self, Command, ExitStatus},
    sys,
};

mod private {
    /// Seals the extension traits, as in std, so litestd may add methods.
    pub trait Sealed {}

    impl Sealed for crate::process::Command {}
    impl Sealed for crate::process::ExitStatus {}
}

/// Unix-specific extensions to the [`process::Command`] builder.
///
/// This trait is sealed: it cannot be implemented outside litestd.
pub trait CommandExt: private::Sealed {
    /// Sets the child process's user ID. This translates to a `setuid` call
    /// in the child process, whose failure makes the spawn fail.
    ///
    /// The child also calls `setgroups(0, NULL)`, removing supplementary
    /// groups that might have given it unwanted permissions.
    fn uid(&mut self, id: u32) -> &mut Command;

    /// Similar to `uid`, but sets the group ID of the child process.
    fn gid(&mut self, id: u32) -> &mut Command;

    /// Schedules a closure to be run just before the `exec` function is
    /// invoked.
    ///
    /// The closure may return an I/O error whose OS error code is
    /// communicated back to the parent and returned by the spawn. Closures
    /// run in order of registration; after an `Err`, no further closure runs.
    ///
    /// # Safety
    ///
    /// The closure runs in the child after a `fork`, where other threads may
    /// have held locks: it must only make async-signal-safe calls, so not
    /// allocate (which excludes `io::Error::new` and `io::Error::other`), take
    /// a mutex, or read the environment through `env`. It must not misuse the
    /// resources, such as descriptors, that the child duplicated. When it
    /// runs, the standard streams and working directory are already
    /// changed. A panic in it aborts the child.
    unsafe fn pre_exec<F>(&mut self, f: F) -> &mut Command
    where
        F: FnMut() -> io::Result<()> + Send + Sync + 'static;

    /// Performs all the required setup by this `Command`, followed by calling
    /// the `execvp` syscall.
    ///
    /// On success this function will not return, and otherwise it will
    /// return an error indicating why the exec (or another part of the setup
    /// of the `Command`) failed. As with [`process::exit`], no destructors
    /// run. It does not fork, and the standard streams are inherited by
    /// default. After a failure the process may be in a broken state: its
    /// working directory, user, streams and signal settings may have
    /// changed.
    #[must_use]
    fn exec(&mut self) -> io::Error;

    /// Set executable argument
    ///
    /// Set the first process argument, `argv[0]`, to something other than
    /// the default executable path.
    fn arg0<S>(&mut self, arg: S) -> &mut Command
    where
        S: AsRef<OsStr>;

    /// Sets the process group ID (PGID) of the child process. Equivalent to
    /// a `setpgid` call in the child process, but may be more efficient.
    ///
    /// Process groups determine which processes receive signals, such as
    /// the `SIGINT` of Ctrl-C in a terminal. A process group ID of 0 will use
    /// the process ID as the PGID.
    fn process_group(&mut self, pgroup: i32) -> &mut Command;
}

impl CommandExt for Command {
    fn uid(&mut self, id: u32) -> &mut Command {
        self.as_inner_mut().uid(id);
        self
    }

    fn gid(&mut self, id: u32) -> &mut Command {
        self.as_inner_mut().gid(id);
        self
    }

    unsafe fn pre_exec<F>(&mut self, f: F) -> &mut Command
    where
        F: FnMut() -> io::Result<()> + Send + Sync + 'static,
    {
        self.as_inner_mut().pre_exec(Box::new(f));
        self
    }

    fn exec(&mut self) -> io::Error {
        self.as_inner_mut().exec(&sys::process::Stdio::Inherit)
    }

    fn arg0<S>(&mut self, arg: S) -> &mut Command
    where
        S: AsRef<OsStr>,
    {
        self.as_inner_mut().set_arg0(arg.as_ref());
        self
    }

    fn process_group(&mut self, pgroup: i32) -> &mut Command {
        self.as_inner_mut().pgroup(pgroup);
        self
    }
}

/// Unix-specific extensions to [`process::ExitStatus`].
///
/// On Unix, an `ExitStatus` is a wait status, as returned by one of the
/// `wait` family of system calls, not just the exit status passed to
/// `exit`. This trait is sealed: it cannot be implemented outside litestd.
pub trait ExitStatusExt: private::Sealed {
    /// Creates a new `ExitStatus` from the raw underlying integer status
    /// value from `wait`: a wait status, not an exit status.
    fn from_raw(raw: i32) -> Self;

    /// If the process was terminated by a signal, returns that signal
    /// (`WTERMSIG`).
    fn signal(&self) -> Option<i32>;

    /// If the process was terminated by a signal, says whether it dumped
    /// core.
    fn core_dumped(&self) -> bool;

    /// If the process was stopped by a signal, returns that signal
    /// (`WSTOPSIG`), which only a wait with `WUNTRACED` reports.
    fn stopped_signal(&self) -> Option<i32>;

    /// Whether the process was continued from a stopped status
    /// (`WIFCONTINUED`), which only a wait with `WCONTINUED` reports.
    fn continued(&self) -> bool;

    /// Returns the underlying raw `wait` status: a wait status, not an exit
    /// status.
    fn into_raw(self) -> i32;
}

impl ExitStatusExt for ExitStatus {
    fn from_raw(raw: i32) -> Self {
        Self(sys::process::ExitStatus::from_raw(raw))
    }

    fn signal(&self) -> Option<i32> {
        self.0.signal()
    }

    fn core_dumped(&self) -> bool {
        self.0.core_dumped()
    }

    fn stopped_signal(&self) -> Option<i32> {
        self.0.stopped_signal()
    }

    fn continued(&self) -> bool {
        self.0.continued()
    }

    fn into_raw(self) -> i32 {
        self.0.into_raw()
    }
}

/// Returns the OS-assigned process identifier associated with this
/// process's parent.
#[must_use]
pub fn parent_id() -> u32 {
    sys::process::getppid()
}

impl FromRawFd for process::Stdio {
    #[inline]
    unsafe fn from_raw_fd(fd: RawFd) -> Self {
        // SAFETY: the caller hands over an open descriptor that nothing else
        // owns.
        Self::from(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

impl From<OwnedFd> for process::Stdio {
    /// Takes ownership of a file descriptor and returns a
    /// [`Stdio`](process::Stdio) that can attach a stream to it.
    #[inline]
    fn from(fd: OwnedFd) -> Self {
        Self(sys::process::Stdio::from(sys::process::ChildPipe::from(fd)))
    }
}

macro_rules! impl_pipe_fd {
    ($($t:ident)*) => {$(
        impl AsRawFd for process::$t {
            #[inline]
            fn as_raw_fd(&self) -> RawFd {
                self.inner.raw()
            }
        }

        impl IntoRawFd for process::$t {
            #[inline]
            fn into_raw_fd(self) -> RawFd {
                self.inner.into_fd().into_raw_fd()
            }
        }

        impl AsFd for process::$t {
            #[inline]
            fn as_fd(&self) -> BorrowedFd<'_> {
                self.inner.as_fd()
            }
        }

        impl From<process::$t> for OwnedFd {
            #[doc = concat!(
                "Takes ownership of a [`", stringify!($t), "`](process::",
                stringify!($t), ")'s file descriptor."
            )]
            #[inline]
            fn from(pipe: process::$t) -> Self {
                pipe.inner.into()
            }
        }

        impl From<OwnedFd> for process::$t {
            #[doc = concat!(
                "Creates a [`", stringify!($t), "`](process::",
                stringify!($t), ") from `fd`, which must be a pipe with ",
                "`CLOEXEC` set."
            )]
            #[inline]
            fn from(fd: OwnedFd) -> Self {
                Self { inner: fd.into() }
            }
        }
    )*};
}

impl_pipe_fd! { ChildStdin ChildStdout ChildStderr }
