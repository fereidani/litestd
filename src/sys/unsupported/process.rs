//! Child processes where the platform has none, as in std: a `Command`
//! records what it is given, and spawning fails.

use core::{convert::Infallible, fmt};

use alloc_crate::vec::Vec;

use super::UNSUPPORTED;
#[cfg(feature = "fs")]
use crate::fs;
pub(crate) use crate::sys::pipe::Pipe as ChildPipe;
use crate::{
    ffi::{OsStr, OsString},
    io,
    path::Path,
    process::{
        StdioPipes,
        args::{Args, ArgsIter},
        env::{CommandEnv, EnvIter},
    },
};

/// Environment variable names, which compare byte by byte.
pub(crate) type EnvKey = OsString;

/// A process builder.
pub(crate) struct Command {
    /// The program, then the arguments.
    args: Args,
    env: CommandEnv,
    cwd: Option<OsString>,
    stdin: Option<Stdio>,
    stdout: Option<Stdio>,
    stderr: Option<Stdio>,
}

/// What a child's standard stream would be connected to.
pub(crate) enum Stdio {
    Inherit,
    Null,
    MakePipe,
    #[cfg(feature = "stdio")]
    ParentStdout,
    #[cfg(feature = "stdio")]
    ParentStderr,
    Fd(ChildPipe),
    #[cfg(feature = "fs")]
    File(fs::File),
}

impl Stdio {
    /// This process's stdout.
    #[cfg(feature = "stdio")]
    pub(crate) const STDOUT: Self = Self::ParentStdout;

    /// This process's stderr.
    #[cfg(feature = "stdio")]
    pub(crate) const STDERR: Self = Self::ParentStderr;
}

impl Command {
    pub(crate) fn new(program: &OsStr) -> Self {
        Self {
            args: Args::new(program),
            env: CommandEnv::default(),
            cwd: None,
            stdin: None,
            stdout: None,
            stderr: None,
        }
    }

    pub(crate) fn arg(&mut self, arg: &OsStr) {
        self.args.push(arg);
    }

    pub(crate) fn env_mut(&mut self) -> &mut CommandEnv {
        &mut self.env
    }

    pub(crate) fn cwd(&mut self, dir: &OsStr) {
        self.cwd = Some(dir.to_os_string());
    }

    pub(crate) fn stdin(&mut self, stdin: Stdio) {
        self.stdin = Some(stdin);
    }

    pub(crate) fn stdout(&mut self, stdout: Stdio) {
        self.stdout = Some(stdout);
    }

    pub(crate) fn stderr(&mut self, stderr: Stdio) {
        self.stderr = Some(stderr);
    }

    pub(crate) fn get_program(&self) -> &OsStr {
        self.args.program()
    }

    pub(crate) fn get_args(&self) -> ArgsIter<'_> {
        self.args.iter()
    }

    pub(crate) fn get_envs(&self) -> EnvIter<'_> {
        self.env.iter()
    }

    pub(crate) fn get_current_dir(&self) -> Option<&Path> {
        self.cwd.as_deref().map(Path::new)
    }

    /// Fails: the platform cannot start processes.
    #[allow(clippy::needless_pass_by_value, reason = "std's internal API")]
    #[allow(clippy::unused_self, reason = "the other backends need it")]
    pub(crate) fn spawn(
        &mut self,
        _default: Stdio,
        _needs_stdin: bool,
    ) -> io::Result<(Process, StdioPipes)> {
        Err(UNSUPPORTED)
    }
}

impl From<ChildPipe> for Stdio {
    fn from(pipe: ChildPipe) -> Self {
        Self::Fd(pipe)
    }
}

#[cfg(feature = "fs")]
impl From<fs::File> for Stdio {
    fn from(file: fs::File) -> Self {
        Self::File(file)
    }
}

impl fmt::Debug for Stdio {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inherit => f.write_str("Inherit"),
            Self::Null => f.write_str("Null"),
            Self::MakePipe => f.write_str("MakePipe"),
            #[cfg(feature = "stdio")]
            Self::ParentStdout => f.write_str("ParentStdout"),
            #[cfg(feature = "stdio")]
            Self::ParentStderr => f.write_str("ParentStderr"),
            Self::Fd(pipe) => f.debug_tuple("Fd").field(pipe).finish(),
            #[cfg(feature = "fs")]
            Self::File(file) => {
                f.debug_tuple("InheritFile").field(file).finish()
            }
        }
    }
}

impl fmt::Debug for Command {
    /// Formats the command as std does on platforms without processes: as a
    /// shell command line, or with `{:#?}` as a structure.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let args: Vec<&OsStr> = self.args.entries().collect();
        if f.alternate() {
            let mut command = f.debug_struct("Command");
            command
                .field("program", &self.get_program())
                .field("args", &args);
            if !self.env.is_unchanged() {
                command.field("env", &self.env);
            }
            if self.cwd.is_some() {
                command.field("cwd", &self.cwd);
            }
            for (name, stdio) in [
                ("stdin", &self.stdin),
                ("stdout", &self.stdout),
                ("stderr", &self.stderr),
            ] {
                if stdio.is_some() {
                    command.field(name, stdio);
                }
            }
            return command.finish();
        }
        if let Some(cwd) = &self.cwd {
            write!(f, "cd {cwd:?} && ")?;
        }
        if self.env.does_clear() {
            f.write_str("env -i ")?;
        } else {
            let mut removed = self.get_envs().filter(|(_, v)| v.is_none());
            if let Some((key, _)) = removed.next() {
                write!(f, "env -u {} ", key.to_string_lossy())?;
            }
            for (key, _) in removed {
                write!(f, "-u {} ", key.to_string_lossy())?;
            }
        }
        for (key, value) in self.get_envs() {
            if let Some(value) = value {
                write!(f, "{}={value:?} ", key.to_string_lossy())?;
            }
        }
        let mut entries = args.iter();
        if let Some(program) = entries.next() {
            write!(f, "{program:?}")?;
        }
        for arg in entries {
            write!(f, " {arg:?}")?;
        }
        Ok(())
    }
}

/// A child process, of which there are none.
pub(crate) struct Process(Infallible);

impl Process {
    pub(crate) fn id(&self) -> u32 {
        match self.0 {}
    }

    pub(crate) fn kill(&mut self) -> io::Result<()> {
        match self.0 {}
    }

    pub(crate) fn wait(&mut self) -> io::Result<ExitStatus> {
        match self.0 {}
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        match self.0 {}
    }
}

/// The status of a process that exited, which std reports as a success
/// with code 0 on platforms without processes.
#[derive(PartialEq, Eq, Clone, Copy, Debug, Default)]
pub(crate) struct ExitStatus;

impl ExitStatus {
    pub(crate) fn success(self) -> bool {
        true
    }

    pub(crate) fn code(self) -> Option<i32> {
        Some(0)
    }
}

impl fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<dummy exit status>")
    }
}

/// Reads the two pipes of a child, of which there are none.
#[allow(clippy::needless_pass_by_value, reason = "std's internal API")]
pub(crate) fn read_output(
    _out: ChildPipe,
    _stdout: &mut Vec<u8>,
    _err: ChildPipe,
    _stderr: &mut Vec<u8>,
) -> io::Result<()> {
    Err(UNSUPPORTED)
}
