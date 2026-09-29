//! [`Command`], the process builder, and the iterators of its getters.

use core::fmt;

use super::{
    Child, ExitStatus, Output, Stdio, args::ArgsIter, child::read_output,
    env::EnvIter,
};
use crate::{ffi::OsStr, io, path::Path, sys::process as imp};

/// A process builder, providing fine-grained control over how a new process
/// should be spawned.
///
/// `Command::new(program)` gives a default configuration, which the builder
/// methods change before spawning. A `Command` can spawn any number of
/// processes, and can be changed between spawns.
///
/// ```no_run
/// use litestd::process::Command;
///
/// let output = Command::new("sh")
///     .arg("-c")
///     .arg("echo hello")
///     .output()
///     .expect("failed to execute process");
/// assert_eq!(output.stdout, b"hello\n");
/// ```
pub struct Command {
    inner: imp::Command,
}

impl Command {
    /// Constructs a new `Command` for launching the program at path
    /// `program`, with no arguments, and the current process's environment,
    /// working directory and standard streams, except that
    /// [`output`](Self::output) captures stdout and stderr.
    ///
    /// A `program` that is not an absolute path is searched for in the
    /// child's `PATH` on Unix: a `PATH` removed by `env_clear` or
    /// `env_remove` means the C library's default list, not the parent's
    /// `PATH`. Windows searches as std does: an explicitly set `PATH`, the
    /// executable's directory, the system directories, then the parent's
    /// `PATH`. `program` is only a path; arguments go through `arg`.
    pub fn new<S: AsRef<OsStr>>(program: S) -> Self {
        Self {
            inner: imp::Command::new(program.as_ref()),
        }
    }

    /// Adds an argument to pass to the program.
    ///
    /// Only one argument can be passed per use, and it reaches the program
    /// literally, not through a shell: quotes, escapes, globs and variables
    /// have no effect. To pass several arguments see [`args`](Self::args).
    pub fn arg<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Self {
        self.inner.arg(arg.as_ref());
        self
    }

    /// Adds multiple arguments to pass to the program, each literally, as by
    /// [`arg`](Self::arg).
    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for arg in args {
            self.arg(arg.as_ref());
        }
        self
    }

    /// Inserts or updates an explicit environment variable mapping.
    ///
    /// Explicitly set variables take precedence over inherited ones. Names
    /// are case-insensitive, but case-preserving, on Windows, and
    /// case-sensitive elsewhere.
    pub fn env<K, V>(&mut self, key: K, val: V) -> &mut Self
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.inner.env_mut().set(key.as_ref(), val.as_ref());
        self
    }

    /// Inserts or updates multiple explicit environment variable mappings,
    /// as by [`env`](Self::env).
    pub fn envs<I, K, V>(&mut self, vars: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        for (ref key, ref val) in vars {
            self.inner.env_mut().set(key.as_ref(), val.as_ref());
        }
        self
    }

    /// Removes an explicitly set environment variable and prevents
    /// inheriting it from a parent process.
    ///
    /// [`get_envs`](Self::get_envs) then yields `None` as its value.
    pub fn env_remove<K: AsRef<OsStr>>(&mut self, key: K) -> &mut Self {
        self.inner.env_mut().remove(key.as_ref());
        self
    }

    /// Clears all explicitly set environment variables and prevents
    /// inheriting any parent process environment variables.
    ///
    /// [`get_envs`](Self::get_envs) is empty afterwards.
    pub fn env_clear(&mut self) -> &mut Self {
        self.inner.env_mut().clear();
        self
    }

    /// Sets the working directory for the child process.
    ///
    /// Whether a relative program path is resolved against this directory
    /// or the parent's is platform-specific; use an absolute program path to
    /// avoid the ambiguity.
    pub fn current_dir<P: AsRef<Path>>(&mut self, dir: P) -> &mut Self {
        self.inner.cwd(dir.as_ref().as_os_str());
        self
    }

    /// Configuration for the child process's standard input (stdin) handle.
    ///
    /// Defaults to [`inherit`](Stdio::inherit) when used with
    /// [`spawn`](Self::spawn) or [`status`](Self::status), and defaults to
    /// [`piped`](Stdio::piped) when used with [`output`](Self::output).
    pub fn stdin<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.inner.stdin(cfg.into().0);
        self
    }

    /// Configuration for the child process's standard output (stdout)
    /// handle, with the defaults of [`stdin`](Self::stdin).
    pub fn stdout<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.inner.stdout(cfg.into().0);
        self
    }

    /// Configuration for the child process's standard error (stderr) handle,
    /// with the defaults of [`stdin`](Self::stdin).
    pub fn stderr<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.inner.stderr(cfg.into().0);
        self
    }

    /// Executes the command as a child process, returning a handle to it.
    ///
    /// By default, stdin, stdout and stderr are inherited from the parent.
    ///
    /// # Errors
    ///
    /// Fails if the child process could not be spawned: for example, the
    /// program was not found or may not be executed, the OS is out of
    /// resources, or a string holds a NUL byte (`InvalidInput`). What
    /// happens to the child once it runs is reported through its
    /// [`ExitStatus`], not as an error.
    pub fn spawn(&mut self) -> io::Result<Child> {
        let (handle, pipes) = self.inner.spawn(imp::Stdio::Inherit, true)?;
        Ok(Child::new(handle, pipes))
    }

    /// Executes the command as a child process, waiting for it to finish and
    /// collecting all of its output.
    ///
    /// By default, stdout and stderr are captured (and used to provide the
    /// resulting output). Stdin is not inherited from the parent: a child
    /// reading from it sees the stream closed immediately. This function
    /// blocks the calling thread.
    ///
    /// # Errors
    ///
    /// Fails as [`spawn`](Self::spawn) does, or if reading the output or
    /// waiting fails. A child that runs and exits unsuccessfully, or is
    /// killed by a signal, is not an error: see [`Output::status`]. Where
    /// std panics on a failed read, litestd returns the error.
    pub fn output(&mut self) -> io::Result<Output> {
        let (mut handle, mut pipes) =
            self.inner.spawn(imp::Stdio::MakePipe, false)?;
        pipes.stdin = None;
        let (stdout, stderr) = read_output(pipes.stdout, pipes.stderr)?;
        let status = ExitStatus(handle.wait()?);
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }

    /// Executes a command as a child process, waiting for it to finish and
    /// collecting its status.
    ///
    /// By default, stdin, stdout and stderr are inherited from the parent.
    /// This function blocks the calling thread.
    ///
    /// # Errors
    ///
    /// Fails as [`spawn`](Self::spawn) does, or if waiting fails; an
    /// unsuccessful exit is reported through the returned [`ExitStatus`].
    pub fn status(&mut self) -> io::Result<ExitStatus> {
        self.spawn()?.wait()
    }

    /// Returns the path to the program that was given to [`Command::new`].
    #[must_use]
    pub fn get_program(&self) -> &OsStr {
        self.inner.get_program()
    }

    /// Returns an iterator of the arguments that will be passed to the
    /// program.
    ///
    /// This does not include the path to the program as the first argument;
    /// it only includes the arguments specified with [`Command::arg`] and
    /// [`Command::args`].
    pub fn get_args(&self) -> CommandArgs<'_> {
        CommandArgs {
            inner: self.inner.get_args(),
        }
    }

    /// Returns an iterator of the environment variables explicitly set for
    /// the child process, sorted by name.
    ///
    /// A `None` value marks a variable removed by
    /// [`env_remove`](Self::env_remove). Inherited variables are not
    /// included, and after [`env_clear`](Self::env_clear) the iterator is
    /// empty.
    pub fn get_envs(&self) -> CommandEnvs<'_> {
        CommandEnvs {
            iter: self.inner.get_envs(),
        }
    }

    /// Returns the working directory for the child process.
    ///
    /// This returns [`None`] if the working directory will not be changed.
    #[must_use]
    pub fn get_current_dir(&self) -> Option<&Path> {
        self.inner.get_current_dir()
    }

    /// The backend's command, for the `os` extension traits.
    #[cfg_attr(not(unix), allow(dead_code, reason = "for the os extensions"))]
    pub(crate) const fn as_inner_mut(&mut self) -> &mut imp::Command {
        &mut self.inner
    }
}

impl fmt::Debug for Command {
    /// Formats the program and arguments of a `Command` for display,
    /// approximating a shell invocation, as std does; the alternate form
    /// shows the other settings too. The format is platform-specific.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.inner, f)
    }
}

/// An iterator over the command arguments.
///
/// This struct is created by [`Command::get_args`].
#[must_use = "iterators are lazy and do nothing unless consumed"]
#[derive(Debug)]
pub struct CommandArgs<'a> {
    inner: ArgsIter<'a>,
}

impl<'a> Iterator for CommandArgs<'a> {
    type Item = &'a OsStr;

    fn next(&mut self) -> Option<&'a OsStr> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for CommandArgs<'_> {
    fn len(&self) -> usize {
        self.inner.len()
    }
}

/// An iterator over the command environment variables.
///
/// This struct is created by [`Command::get_envs`].
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct CommandEnvs<'a> {
    iter: EnvIter<'a>,
}

impl<'a> Iterator for CommandEnvs<'a> {
    type Item = (&'a OsStr, Option<&'a OsStr>);

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl ExactSizeIterator for CommandEnvs<'_> {
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl fmt::Debug for CommandEnvs<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandEnvs")
            .field("iter", &self.iter)
            .finish()
    }
}
