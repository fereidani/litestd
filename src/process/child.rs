//! [`Child`], its standard stream handles, and [`Output`].

use core::{fmt, str};

use alloc_crate::{string::String, vec::Vec};

use super::ExitStatus;
use crate::{
    io::{self, IoSlice, IoSliceMut, Read, Write},
    sys::process::{self as imp, ChildPipe},
};

/// Representation of a running or exited child process.
///
/// A child process is created via [`Command`](super::Command). There is no
/// implementation of [`Drop`] for child processes, so a `Child` that goes
/// out of scope before its process ends leaves the process running.
///
/// # Warning
///
/// On some systems, calling [`wait`](Child::wait) or similar is necessary
/// for the OS to release resources: a process that terminated but has not
/// been waited on stays around as a "zombie". litestd, like std, never
/// waits on a child implicitly, not even when the `Child` is dropped.
pub struct Child {
    pub(crate) handle: imp::Process,

    /// The handle for writing to the child's standard input (stdin), if it
    /// has been captured. Use `child.stdin.take()` to use it without
    /// partially moving `child`.
    pub stdin: Option<ChildStdin>,

    /// The handle for reading from the child's standard output (stdout), if
    /// it has been captured. Use `child.stdout.take()` to use it without
    /// partially moving `child`.
    pub stdout: Option<ChildStdout>,

    /// The handle for reading from the child's standard error (stderr), if
    /// it has been captured. Use `child.stderr.take()` to use it without
    /// partially moving `child`.
    pub stderr: Option<ChildStderr>,
}

/// The parent's ends of the pipes a spawned process was given.
pub(crate) struct StdioPipes {
    pub(crate) stdin: Option<ChildPipe>,
    pub(crate) stdout: Option<ChildPipe>,
    pub(crate) stderr: Option<ChildPipe>,
}

impl Child {
    pub(crate) fn new(handle: imp::Process, pipes: StdioPipes) -> Self {
        Self {
            handle,
            stdin: pipes.stdin.map(|inner| ChildStdin { inner }),
            stdout: pipes.stdout.map(|inner| ChildStdout { inner }),
            stderr: pipes.stderr.map(|inner| ChildStderr { inner }),
        }
    }

    /// Forces the child process to exit. If the child has already exited,
    /// `Ok(())` is returned.
    ///
    /// This is equivalent to sending a `SIGKILL` on Unix platforms. The
    /// mapping of errors to [`ErrorKind`](io::ErrorKind)s is not part of the
    /// compatibility contract of the function.
    ///
    /// # Errors
    ///
    /// Fails if the OS cannot deliver the signal.
    pub fn kill(&mut self) -> io::Result<()> {
        self.handle.kill()
    }

    /// Returns the OS-assigned process identifier associated with this
    /// child.
    #[must_use]
    #[allow(clippy::missing_const_for_fn, reason = "not `const` in std")]
    pub fn id(&self) -> u32 {
        self.handle.id()
    }

    /// Waits for the child to exit completely, returning the status that it
    /// exited with. This function will continue to have the same return
    /// value after it has been called at least once.
    ///
    /// The stdin handle to the child process, if any, is closed before
    /// waiting, so that the child cannot block reading from the parent while
    /// the parent waits for it. This function blocks the calling thread.
    ///
    /// # Errors
    ///
    /// Fails if the OS cannot wait for the child.
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.stdin = None;
        self.handle.wait().map(ExitStatus)
    }

    /// Attempts to collect the exit status of the child if it has already
    /// exited, without blocking.
    ///
    /// Returns `Ok(Some(status))` once the child has exited, which on Unix
    /// reaps it, and `Ok(None)` while it runs. Unlike `wait`, this does not
    /// close stdin.
    ///
    /// # Errors
    ///
    /// Fails if the OS cannot query the child.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        Ok(self.handle.try_wait()?.map(ExitStatus))
    }

    /// Simultaneously waits for the child to exit and collect all remaining
    /// output on the stdout/stderr handles, returning an `Output` instance.
    ///
    /// The stdin handle is closed first, as by [`wait`](Child::wait). Only
    /// the streams configured with [`Stdio::piped`](super::Stdio::piped) are
    /// captured. This function blocks the calling thread.
    ///
    /// # Errors
    ///
    /// Fails if reading the output or waiting fails. Where std panics on a
    /// failed read, litestd returns the error, without waiting.
    pub fn wait_with_output(mut self) -> io::Result<Output> {
        self.stdin = None;
        let (stdout, stderr) = read_output(
            self.stdout.take().map(|pipe| pipe.inner),
            self.stderr.take().map(|pipe| pipe.inner),
        )?;
        let status = self.wait()?;
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }
}

/// Reads the captured streams of a child to their ends.
pub(crate) fn read_output(
    out: Option<ChildPipe>,
    err: Option<ChildPipe>,
) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    match (out, err) {
        (None, None) => {}
        (Some(out), None) => {
            out.read_to_end(&mut stdout)?;
        }
        (None, Some(err)) => {
            err.read_to_end(&mut stderr)?;
        }
        (Some(out), Some(err)) => {
            imp::read_output(out, &mut stdout, err, &mut stderr)?;
        }
    }
    Ok((stdout, stderr))
}

impl fmt::Debug for Child {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Child")
            .field("stdin", &self.stdin)
            .field("stdout", &self.stdout)
            .field("stderr", &self.stderr)
            .finish_non_exhaustive()
    }
}

/// A handle to a child process's standard input (stdin).
///
/// This struct is used in the [`stdin`](Child::stdin) field on [`Child`].
/// Dropping it closes the pipe, which unblocks a child waiting for input.
///
/// On Unix, writing after the child closed its end fails with
/// [`BrokenPipe`](crate::io::ErrorKind::BrokenPipe), as in std: litestd
/// ignores `SIGPIPE` at startup, as std's runtime does.
pub struct ChildStdin {
    pub(crate) inner: ChildPipe,
}

/// A handle to a child process's standard output (stdout).
///
/// This struct is used in the [`stdout`](Child::stdout) field on [`Child`].
/// Dropping it closes the pipe.
pub struct ChildStdout {
    pub(crate) inner: ChildPipe,
}

/// A handle to a child process's stderr.
///
/// This struct is used in the [`stderr`](Child::stderr) field on [`Child`].
/// Dropping it closes the pipe.
pub struct ChildStderr {
    pub(crate) inner: ChildPipe,
}

impl Write for ChildStdin {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.inner.write_vectored(bufs)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Write for &ChildStdin {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.inner.write_vectored(bufs)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

macro_rules! impl_read {
    ($($t:ident)*) => {$(
        impl Read for $t {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                self.inner.read(buf)
            }

            fn read_vectored(
                &mut self,
                bufs: &mut [IoSliceMut<'_>],
            ) -> io::Result<usize> {
                self.inner.read_vectored(bufs)
            }

            fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
                self.inner.read_to_end(buf)
            }

            fn read_to_string(&mut self, buf: &mut String) -> io::Result<usize> {
                self.inner.read_to_string(buf)
            }
        }
    )*};
}

impl_read! { ChildStdout ChildStderr }

macro_rules! impl_debug {
    ($($t:ident)*) => {$(
        impl fmt::Debug for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($t)).finish_non_exhaustive()
            }
        }
    )*};
}

impl_debug! { ChildStdin ChildStdout ChildStderr }

/// The output of a finished process.
///
/// This is returned by [`Command::output`](super::Command::output) and
/// [`Child::wait_with_output`].
#[derive(PartialEq, Eq, Clone)]
pub struct Output {
    /// The status (exit code) of the process.
    pub status: ExitStatus,
    /// The data that the process wrote to stdout.
    pub stdout: Vec<u8>,
    /// The data that the process wrote to stderr.
    pub stderr: Vec<u8>,
}

impl fmt::Debug for Output {
    /// Shows each stream as a string if it is UTF-8, and as bytes otherwise.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        /// A stream as `Output`'s `Debug` shows it.
        struct Stream<'a>(&'a [u8]);

        impl fmt::Debug for Stream<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match str::from_utf8(self.0) {
                    Ok(s) => fmt::Debug::fmt(s, f),
                    Err(_) => fmt::Debug::fmt(self.0, f),
                }
            }
        }

        f.debug_struct("Output")
            .field("status", &self.status)
            .field("stdout", &Stream(&self.stdout))
            .field("stderr", &Stream(&self.stderr))
            .finish()
    }
}
