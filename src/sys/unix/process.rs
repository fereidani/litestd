//! Child processes: `posix_spawn` where the command allows it, as in std,
//! and `fork` and `exec` otherwise.

mod environ;
mod fd;
mod fork;
#[cfg(any(
    all(target_os = "linux", any(target_env = "gnu", target_env = "musl")),
    target_vendor = "apple",
    target_os = "freebsd"
))]
mod posix_spawn;
mod status;

use core::{
    ffi::{CStr, c_char, c_int},
    fmt, ptr,
};

use alloc_crate::{boxed::Box, vec::Vec};

use self::environ::{Candidates, EnvBlock, EnvLock};
pub(crate) use self::{
    fd::read_output,
    status::{ExitStatus, Process},
};
pub(crate) use crate::sys::pipe::Pipe as ChildPipe;
use crate::{
    ffi::{OsStr, OsString},
    io,
    path::Path,
    process::{
        StdioPipes,
        args::{Args, ArgsIter, c_str_or_placeholder},
        env::{CommandEnv, EnvIter},
    },
    sys::{os, pipe},
};
#[cfg(feature = "fs")]
use crate::{fs, os::fd::OwnedFd};

/// Environment variable names, which compare byte by byte on Unix.
pub(crate) type EnvKey = OsString;

/// A closure that `pre_exec` schedules to run in the child.
pub(crate) type PreExec = Box<dyn FnMut() -> io::Result<()> + Send + Sync>;

/// A process builder.
pub(crate) struct Command {
    /// The program, then the arguments.
    args: Args,
    env: CommandEnv,
    /// The working directory, followed by a NUL byte.
    cwd: Option<Vec<u8>>,
    /// The program's name for itself, followed by a NUL byte.
    arg0: Option<Vec<u8>>,
    ids: Ids,
    closures: Vec<PreExec>,
    stdin: Option<Stdio>,
    stdout: Option<Stdio>,
    stderr: Option<Stdio>,
}

/// The user, group and process group a child is given, if set.
#[derive(Clone, Copy, Default)]
struct Ids {
    uid: Option<libc::uid_t>,
    gid: Option<libc::gid_t>,
    pgroup: Option<libc::pid_t>,
}

/// What a child's standard stream is connected to.
pub(crate) enum Stdio {
    Inherit,
    Null,
    MakePipe,
    /// A descriptor, which each child spawned gets a copy of.
    Fd(ChildPipe),
    /// This process's stdout or stderr, which `io::Stdout` and `io::Stderr`
    /// stand for; each child gets a copy.
    #[cfg(feature = "stdio")]
    StaticFd(c_int),
}

#[cfg(feature = "stdio")]
impl Stdio {
    /// This process's stdout.
    pub(crate) const STDOUT: Self = Self::StaticFd(libc::STDOUT_FILENO);

    /// This process's stderr.
    pub(crate) const STDERR: Self = Self::StaticFd(libc::STDERR_FILENO);
}

impl Command {
    pub(crate) fn new(program: &OsStr) -> Self {
        Self {
            args: Args::new(program),
            env: CommandEnv::default(),
            cwd: None,
            arg0: None,
            ids: Ids::default(),
            closures: Vec::new(),
            stdin: None,
            stdout: None,
            stderr: None,
        }
    }

    pub(crate) fn arg(&mut self, arg: &OsStr) {
        self.args.push(arg);
    }

    pub(crate) const fn env_mut(&mut self) -> &mut CommandEnv {
        &mut self.env
    }

    pub(crate) fn cwd(&mut self, dir: &OsStr) {
        self.cwd = Some(nul_terminated(dir));
    }

    pub(crate) fn set_arg0(&mut self, arg0: &OsStr) {
        self.arg0 = Some(nul_terminated(arg0));
    }

    pub(crate) const fn uid(&mut self, uid: libc::uid_t) {
        self.ids.uid = Some(uid);
    }

    pub(crate) const fn gid(&mut self, gid: libc::gid_t) {
        self.ids.gid = Some(gid);
    }

    pub(crate) const fn pgroup(&mut self, pgroup: libc::pid_t) {
        self.ids.pgroup = Some(pgroup);
    }

    pub(crate) fn pre_exec(&mut self, f: PreExec) {
        self.closures.push(f);
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
        let cwd = self.cwd.as_deref()?;
        let cwd = cwd.split_last().map_or(cwd, |(_, cwd)| cwd);
        Some(Path::new(OsStr::from_unix_bytes(cwd)))
    }

    /// Starts the program, with `default` for the streams not configured,
    /// except that stdin is null if `needs_stdin` is `false`.
    #[allow(clippy::needless_pass_by_value, reason = "std's internal API")]
    pub(crate) fn spawn(
        &mut self,
        default: Stdio,
        needs_stdin: bool,
    ) -> io::Result<(Process, StdioPipes)> {
        let posix_spawn = self.can_posix_spawn();
        // Held until the child exists, as in std: the child inherits
        // `environ` or a copy made from it, and a forked child must see it
        // whole.
        let lock = EnvLock::read();
        let exec = Exec::new(
            &self.args,
            &self.env,
            self.cwd.as_deref(),
            self.arg0.as_deref(),
            &lock,
        )?;
        let (ours, theirs) = self.setup_io(&default, needs_stdin)?;
        let spawned = if posix_spawn {
            exec.posix_spawn(&theirs.fds, self.ids.pgroup)?
        } else {
            None
        };
        let pid = match spawned {
            Some(pid) => pid,
            None => exec.fork(theirs.fds, self.ids, &mut self.closures)?,
        };
        Ok((Process::new(pid), ours))
    }

    /// Sets this process up as the command describes and replaces it with
    /// the program, returning only on failure.
    pub(crate) fn exec(&mut self, default: &Stdio) -> io::Error {
        match self.try_exec(default) {
            Ok(e) | Err(e) => e,
        }
    }

    /// `exec`, with the failures before the setup starts as `Err`.
    fn try_exec(&mut self, default: &Stdio) -> io::Result<io::Error> {
        let lock = EnvLock::read();
        let exec = Exec::new(
            &self.args,
            &self.env,
            self.cwd.as_deref(),
            self.arg0.as_deref(),
            &lock,
        )?;
        let (_ours, theirs) = self.setup_io(default, true)?;
        let candidates = exec.candidates();
        let mut child =
            exec.child(theirs.fds, &candidates, self.ids, &mut self.closures);
        Ok(fork::exec(&mut child))
    }

    /// Whether `posix_spawn` can do what the command asks, as std decides:
    /// not for user or group changes or `pre_exec` closures, nor to search a
    /// changed `PATH`, which `posix_spawnp` ignores. On macOS, not for a
    /// relative program path with a working directory either: the child
    /// starts, but the call fails with `ENOENT`.
    fn can_posix_spawn(&self) -> bool {
        let program = self.get_program().as_encoded_bytes();
        let relative = program.contains(&b'/') && !program.starts_with(b"/");
        cfg!(any(
            all(
                target_os = "linux",
                any(target_env = "gnu", target_env = "musl")
            ),
            target_vendor = "apple",
            target_os = "freebsd"
        )) && self.ids.uid.is_none()
            && self.ids.gid.is_none()
            && self.closures.is_empty()
            && (!self.env.have_changed_path() || program.contains(&b'/'))
            && !(cfg!(target_vendor = "apple")
                && relative
                && self.cwd.is_some())
    }

    /// Creates the descriptors the child gets as its standard streams, and
    /// the parent's ends of the pipes among them.
    fn setup_io(
        &self,
        default: &Stdio,
        needs_stdin: bool,
    ) -> io::Result<(StdioPipes, ChildStdio)> {
        let null = Stdio::Null;
        let stdin_default = if needs_stdin { default } else { &null };
        let mut theirs = ChildStdio::default();
        let stdin =
            theirs.set(0, self.stdin.as_ref().unwrap_or(stdin_default))?;
        let stdout = theirs.set(1, self.stdout.as_ref().unwrap_or(default))?;
        let stderr = theirs.set(2, self.stderr.as_ref().unwrap_or(default))?;
        Ok((
            StdioPipes {
                stdin,
                stdout,
                stderr,
            },
            theirs,
        ))
    }
}

/// Copies `s` with a NUL byte after it: a C string unless `s` holds one.
fn nul_terminated(s: &OsStr) -> Vec<u8> {
    let s = s.as_encoded_bytes();
    let mut bytes = Vec::with_capacity(s.len() + 1);
    bytes.extend_from_slice(s);
    bytes.push(0);
    bytes
}

/// Builds `argv`: pointers to the entries of `args`, with `name` in place of
/// the program's if set, ended by null. The entries hold no NUL byte.
fn argv(args: &Args, name: Option<&CStr>) -> Vec<*const c_char> {
    let mut ptrs = Vec::with_capacity(args.len() + 1);
    let mut entries = args.entries();
    // Every entry ends with a NUL byte, so each pointer is a C string.
    while let Some(entry) = entries.next_with_nul() {
        ptrs.push(entry.as_ptr().cast());
    }
    if let (Some(name), Some(first)) = (name, ptrs.first_mut()) {
        *first = name.as_ptr();
    }
    ptrs.push(ptr::null());
    ptrs
}

/// What running the program takes, built before any descriptor is created.
struct Exec<'a> {
    program: &'a CStr,
    /// The arguments, `arg0` first, ended by null.
    argv: Vec<*const c_char>,
    /// The child's environment, if the command changes it.
    env: Option<EnvBlock>,
    cwd: Option<&'a CStr>,
    lock: &'a EnvLock,
}

impl<'a> Exec<'a> {
    /// Checks the strings and builds what `exec` needs before anything is
    /// created, so that a NUL byte fails with `InvalidInput` first, as in
    /// std.
    fn new(
        args: &'a Args,
        env: &CommandEnv,
        cwd: Option<&'a [u8]>,
        name: Option<&'a [u8]>,
        lock: &'a EnvLock,
    ) -> io::Result<Self> {
        let env = if env.is_unchanged() {
            None
        } else {
            Some(EnvBlock::new(env, lock)?)
        };
        let program = args.entries().next_with_nul().unwrap_or(b"\0");
        let checked = (
            CStr::from_bytes_with_nul(program),
            cwd.map(CStr::from_bytes_with_nul).transpose(),
            name.map(CStr::from_bytes_with_nul).transpose(),
        );
        let (Ok(program), Ok(cwd), Ok(name)) = checked else {
            return Err(os::NUL_IN_DATA);
        };
        if args.saw_nul() {
            return Err(os::NUL_IN_DATA);
        }
        Ok(Self {
            program,
            argv: argv(args, name),
            env,
            cwd,
            lock,
        })
    }

    fn envp(&self) -> *const *const c_char {
        self.env
            .as_ref()
            .map_or_else(|| environ::current(self.lock), EnvBlock::as_ptr)
    }

    #[cfg(any(
        all(target_os = "linux", any(target_env = "gnu", target_env = "musl")),
        target_vendor = "apple",
        target_os = "freebsd"
    ))]
    fn posix_spawn(
        &self,
        stdio: &[c_int; 3],
        pgroup: Option<libc::pid_t>,
    ) -> io::Result<Option<libc::pid_t>> {
        posix_spawn::Spawn {
            program: self.program.as_ptr(),
            argv: self.argv.as_ptr(),
            envp: self.envp(),
            cwd: self.cwd,
            stdio,
            pgroup,
        }
        .run()
    }

    #[cfg(not(any(
        all(target_os = "linux", any(target_env = "gnu", target_env = "musl")),
        target_vendor = "apple",
        target_os = "freebsd"
    )))]
    #[allow(clippy::unused_self, clippy::unnecessary_wraps)]
    const fn posix_spawn(
        &self,
        _: &[c_int; 3],
        _: Option<libc::pid_t>,
    ) -> io::Result<Option<libc::pid_t>> {
        Ok(None)
    }

    /// Lists the paths the child tries: the program itself, or where the
    /// child's `PATH` finds it.
    fn candidates(&self) -> Candidates {
        let program = self.program.to_bytes();
        self.env.as_ref().map_or_else(
            || {
                Candidates::new(
                    program,
                    environ::current_path(self.lock).as_deref(),
                )
            },
            |env| Candidates::new(program, env.path()),
        )
    }

    /// Spawns the child with `fork` and `exec`.
    fn fork(
        &self,
        stdio: [c_int; 3],
        ids: Ids,
        closures: &mut [PreExec],
    ) -> io::Result<libc::pid_t> {
        let candidates = self.candidates();
        fork::spawn(&mut self.child(stdio, &candidates, ids, closures))
    }

    fn child<'b>(
        &'b self,
        stdio: [c_int; 3],
        candidates: &'b Candidates,
        ids: Ids,
        closures: &'b mut [PreExec],
    ) -> fork::Child<'b> {
        fork::Child {
            stdio,
            ids,
            cwd: self.cwd.map(CStr::as_ptr),
            closures,
            argv: &self.argv,
            envp: self.envp(),
            #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
            environ: os::environ_slot(),
            candidates,
            #[cfg(any(target_env = "gnu", target_vendor = "apple"))]
            script_argv: fork::script_argv(&self.argv),
        }
    }
}

/// The descriptors a child gets as its standard streams: `-1` inherits.
/// Each is 3 or above, so that setting one stream never overwrites the
/// source of another.
struct ChildStdio {
    fds: [c_int; 3],
    /// The descriptors created for this child, closed once it is spawned.
    owned: [Option<ChildPipe>; 3],
    /// A write-only `/dev/null` already opened for stdout, which stderr
    /// shares, or `-1`.
    null_out: c_int,
}

impl Default for ChildStdio {
    fn default() -> Self {
        Self {
            fds: [-1; 3],
            owned: [None, None, None],
            null_out: -1,
        }
    }
}

impl ChildStdio {
    /// Connects stream `n` as `cfg` says, returning the parent's end of a
    /// new pipe.
    fn set(&mut self, n: usize, cfg: &Stdio) -> io::Result<Option<ChildPipe>> {
        // The child reads stdin and writes the others.
        let output = n != 0;
        let (fd, ours) = match cfg {
            Stdio::Inherit => return Ok(None),
            Stdio::Null if output && self.null_out >= 0 => {
                if let Some(slot) = self.fds.get_mut(n) {
                    *slot = self.null_out;
                }
                return Ok(None);
            }
            Stdio::Null => (fd::open_null(output)?, None),
            Stdio::MakePipe => {
                let (reader, writer) = pipe::pipe()?;
                if output {
                    (writer, Some(reader))
                } else {
                    (reader, Some(writer))
                }
            }
            Stdio::Fd(fd) if fd.raw() > libc::STDERR_FILENO => {
                if let Some(slot) = self.fds.get_mut(n) {
                    *slot = fd.raw();
                }
                return Ok(None);
            }
            Stdio::Fd(fd) => (fd.try_clone()?, None),
            #[cfg(feature = "stdio")]
            Stdio::StaticFd(fd) => (fd::duplicate_stdio(*fd)?, None),
        };
        // A new descriptor takes a standard stream's number only if this
        // process closed that stream; move it out of the way.
        let fd = if fd.raw() <= libc::STDERR_FILENO {
            fd.try_clone()?
        } else {
            fd
        };
        if output && matches!(cfg, Stdio::Null) {
            self.null_out = fd.raw();
        }
        if let (Some(slot), Some(owned)) =
            (self.fds.get_mut(n), self.owned.get_mut(n))
        {
            *slot = fd.raw();
            *owned = Some(fd);
        }
        Ok(ours)
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
        Self::Fd(ChildPipe::from(OwnedFd::from(file)))
    }
}

impl fmt::Debug for Stdio {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inherit => f.write_str("Inherit"),
            Self::Null => f.write_str("Null"),
            Self::MakePipe => f.write_str("MakePipe"),
            Self::Fd(fd) => f.debug_tuple("Fd").field(fd).finish(),
            #[cfg(feature = "stdio")]
            Self::StaticFd(fd) => {
                struct Borrowed(c_int);

                impl fmt::Debug for Borrowed {
                    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        f.debug_struct("BorrowedFd")
                            .field("fd", &self.0)
                            .finish()
                    }
                }

                f.debug_tuple("StaticFd").field(&Borrowed(*fd)).finish()
            }
        }
    }
}

impl fmt::Debug for Command {
    /// Formats the command as std's Unix backend does: as a shell command
    /// line, or with `{:#?}` as a structure.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut entries = self.args.entries();
        let program = entries.next_with_nul().unwrap_or(b"\0");
        let program = c_str_or_placeholder(program);
        let arg0 = self.arg0.as_deref().map_or(program, c_str_or_placeholder);
        if f.alternate() {
            return self.fmt_struct(f, program, arg0);
        }
        if let Some(cwd) = &self.cwd {
            write!(f, "cd {:?} && ", c_str_or_placeholder(cwd))?;
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
        if program != arg0 {
            write!(f, "[{program:?}] ")?;
        }
        write!(f, "{arg0:?}")?;
        for arg in self.get_args() {
            write!(f, " {arg:?}")?;
        }
        Ok(())
    }
}

impl Command {
    /// The `{:#?}` form of `Debug`, with std's fields.
    fn fmt_struct(
        &self,
        f: &mut fmt::Formatter<'_>,
        program: &CStr,
        arg0: &CStr,
    ) -> fmt::Result {
        /// The arguments as std lists them, `arg0` first.
        struct Argv<'a>(&'a CStr, ArgsIter<'a>);

        impl fmt::Debug for Argv<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let mut list = f.debug_list();
                list.entry(&self.0);
                let mut args = self.1.clone();
                while let Some(arg) = args.next_with_nul() {
                    list.entry(&c_str_or_placeholder(arg));
                }
                list.finish()
            }
        }

        let mut s = f.debug_struct("Command");
        s.field("program", &program);
        s.field("args", &Argv(arg0, self.get_args()));
        if !self.env.is_unchanged() {
            s.field("env", &self.env);
        }
        if let Some(cwd) = &self.cwd {
            s.field("cwd", &Some(c_str_or_placeholder(cwd)));
        }
        if self.ids.uid.is_some() {
            s.field("uid", &self.ids.uid);
        }
        if self.ids.gid.is_some() {
            s.field("gid", &self.ids.gid);
        }
        for (name, stdio) in [
            ("stdin", &self.stdin),
            ("stdout", &self.stdout),
            ("stderr", &self.stderr),
        ] {
            if stdio.is_some() {
                s.field(name, stdio);
            }
        }
        if self.ids.pgroup.is_some() {
            s.field("pgroup", &self.ids.pgroup);
        }
        // Only std's Linux commands have the field.
        #[cfg(target_os = "linux")]
        s.field("create_pidfd", &false);
        s.finish()
    }
}

/// Returns the id of this process's parent.
pub(crate) fn getppid() -> u32 {
    // SAFETY: `getppid` has no preconditions and always succeeds.
    let pid = unsafe { libc::getppid() };
    // Process ids are positive, so the conversion never falls back.
    u32::try_from(pid).unwrap_or(0)
}
