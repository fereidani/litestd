//! Child processes, created with `CreateProcessW` as std creates them.

mod args;
mod env;
mod pipe;
mod resolve;

use core::{cell::UnsafeCell, ffi::c_void, fmt, ptr};

use alloc_crate::vec::Vec;
use windows_sys::{
    Win32::{
        Foundation::{
            ERROR_ACCESS_DENIED, GENERIC_READ, GENERIC_WRITE,
            INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        Security::SECURITY_ATTRIBUTES,
        Storage::FileSystem::{
            CreateFileW, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            OPEN_EXISTING,
        },
        System::{
            Console::{
                GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE, STD_INPUT_HANDLE,
                STD_OUTPUT_HANDLE,
            },
            Threading::{
                AcquireSRWLockExclusive, CREATE_UNICODE_ENVIRONMENT,
                CreateProcessW, GetExitCodeProcess, INFINITE,
                PROCESS_INFORMATION, ReleaseSRWLockExclusive, SRWLOCK,
                SRWLOCK_INIT, STARTF_USESTDHANDLES, STARTUPINFOEXW,
                STARTUPINFOW, TerminateProcess, WaitForSingleObject,
            },
        },
    },
    core::w,
};

pub(crate) use self::{
    env::EnvKey,
    pipe::{ChildPipe, read_output},
};
use super::{
    os::{cvt, os_code},
    pipe::Pipe,
};
#[cfg(feature = "fs")]
use crate::fs;
use crate::{
    ffi::{OsStr, OsString},
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, duplicate},
    path::Path,
    process::{
        StdioPipes,
        args::{Args, ArgsIter},
        env::{CommandEnv, EnvIter},
    },
};

/// Serializes the part of spawning in which handles for the child are
/// inheritable, so that no other child that litestd starts inherits them.
static SPAWN_LOCK: SpawnLock = SpawnLock(UnsafeCell::new(SRWLOCK_INIT));

/// An SRW lock that can live in a `static`.
struct SpawnLock(UnsafeCell<SRWLOCK>);

// SAFETY: the lock is only used through the SRW lock functions, which are
// made to be called on one lock from any number of threads.
unsafe impl Sync for SpawnLock {}

/// A process builder.
pub(crate) struct Command {
    /// The program, then the arguments.
    args: Args,
    /// The indices in `args` of the arguments that `raw_arg` added, in
    /// ascending order.
    raw: Vec<usize>,
    env: CommandEnv,
    cwd: Option<OsString>,
    flags: u32,
    stdin: Option<Stdio>,
    stdout: Option<Stdio>,
    stderr: Option<Stdio>,
}

/// What a child's standard stream is connected to.
pub(crate) enum Stdio {
    Inherit,
    /// This process's standard stream with the given id, whichever stream
    /// the child's is.
    #[cfg(feature = "stdio")]
    InheritSpecific(STD_HANDLE),
    Null,
    MakePipe,
    /// A pipe to another child, relayed through a thread.
    Pipe(ChildPipe),
    /// A handle, duplicated for each child.
    Handle(OwnedHandle),
}

impl Command {
    pub(crate) fn new(program: &OsStr) -> Self {
        Self {
            args: Args::new(program),
            raw: Vec::new(),
            env: CommandEnv::default(),
            cwd: None,
            flags: 0,
            stdin: None,
            stdout: None,
            stderr: None,
        }
    }

    pub(crate) fn arg(&mut self, arg: &OsStr) {
        self.args.push(arg);
    }

    /// Adds an argument that goes to the command line as it is.
    pub(crate) fn raw_arg(&mut self, arg: &OsStr) {
        self.raw.push(self.args.len());
        self.args.push(arg);
    }

    pub(crate) const fn env_mut(&mut self) -> &mut CommandEnv {
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

    /// Sets the creation flags, which `spawn` ORs with
    /// `CREATE_UNICODE_ENVIRONMENT`.
    pub(crate) const fn creation_flags(&mut self, flags: u32) {
        self.flags = flags;
    }

    pub(crate) fn get_program(&self) -> &OsStr {
        self.args.program()
    }

    pub(crate) fn get_args(&self) -> ArgsIter<'_> {
        self.args.iter().with_raw(&self.raw)
    }

    pub(crate) fn get_envs(&self) -> EnvIter<'_> {
        self.env.iter()
    }

    pub(crate) fn get_current_dir(&self) -> Option<&Path> {
        self.cwd.as_deref().map(Path::new)
    }

    /// Starts the program, with `default` for the streams not configured,
    /// except that stdin is null if `needs_stdin` is `false`.
    ///
    /// Everything that can fail before the process starts is checked in
    /// std's order: the program, the arguments, the environment and the
    /// working directory.
    #[allow(
        clippy::needless_pass_by_value,
        clippy::needless_pass_by_ref_mut,
        reason = "the signature of every backend, as in std"
    )]
    pub(crate) fn spawn(
        &mut self,
        default: Stdio,
        needs_stdin: bool,
    ) -> io::Result<(Process, StdioPipes)> {
        let changes = self.env.vars();
        // The child's `PATH` is searched first if the command changed it.
        let child_paths = if self.env.have_changed_path() {
            env::find(changes, "PATH")
        } else {
            None
        };
        let program = resolve::resolve_exe(self.get_program(), child_paths)?;
        let (application, mut command_line) =
            if resolve::is_batch_file(&program)? {
                let line = args::batch_command_line(
                    &program,
                    self.get_args(),
                    &self.raw,
                )?;
                (resolve::command_prompt()?, line)
            } else {
                let program_arg = self.get_program();
                let line = args::command_line(
                    program_arg,
                    self.get_args(),
                    &self.raw,
                )?;
                (program, line)
            };
        let env = if self.env.is_unchanged() {
            None
        } else {
            Some(env::env_block(self.env.does_clear(), changes)?)
        };
        let cwd = self.cwd.as_deref().map(resolve::working_dir).transpose()?;
        let mut start = Start {
            application: &application,
            command_line: &mut command_line,
            env: env.as_deref(),
            cwd: cwd
                .as_ref()
                .map(|(dir, start)| dir.get(*start..).unwrap_or_default()),
        };
        // SAFETY: `SPAWN_LOCK` is a valid SRW lock for the life of the
        // process, which only this function takes, and releases below.
        unsafe { AcquireSRWLockExclusive(SPAWN_LOCK.0.get()) };
        let result = self.spawn_locked(&mut start, &default, needs_stdin);
        // SAFETY: this thread acquired the lock above.
        unsafe { ReleaseSRWLockExclusive(SPAWN_LOCK.0.get()) };
        result
    }

    /// The part of `spawn` that holds `SPAWN_LOCK`. The inheritable handles
    /// it makes for the child are closed before it returns.
    fn spawn_locked(
        &self,
        start: &mut Start<'_>,
        default: &Stdio,
        needs_stdin: bool,
    ) -> io::Result<(Process, StdioPipes)> {
        let mut pipes = StdioPipes {
            stdin: None,
            stdout: None,
            stderr: None,
        };
        let null = Stdio::Null;
        let stdin_default = if needs_stdin { default } else { &null };
        let stdin = self.stdin.as_ref().unwrap_or(stdin_default);
        let stdout = self.stdout.as_ref().unwrap_or(default);
        let stderr = self.stderr.as_ref().unwrap_or(default);
        let stdin = stdin.to_handle(STD_INPUT_HANDLE, &mut pipes.stdin)?;
        let stdout = stdout.to_handle(STD_OUTPUT_HANDLE, &mut pipes.stdout)?;
        let stderr = stderr.to_handle(STD_ERROR_HANDLE, &mut pipes.stderr)?;
        // The extended structure, though only its first part is passed, so
        // that even with `EXTENDED_STARTUPINFO_PRESENT` among the creation
        // flags, nothing is read beyond memory this function owns.
        let mut info = STARTUPINFOEXW::default();
        // The structure is a hundred bytes or so.
        #[allow(clippy::cast_possible_truncation)]
        let size = size_of::<STARTUPINFOW>() as u32;
        info.StartupInfo.cb = size;
        // Without any handle, the child gets the OS's defaults, as in std.
        if stdin.is_some() || stdout.is_some() || stderr.is_some() {
            let raw = |h: &Option<OwnedHandle>| {
                h.as_ref()
                    .map_or(ptr::null_mut(), AsRawHandle::as_raw_handle)
            };
            info.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            info.StartupInfo.hStdInput = raw(&stdin);
            info.StartupInfo.hStdOutput = raw(&stdout);
            info.StartupInfo.hStdError = raw(&stderr);
        }
        let env = start
            .env
            .map_or(ptr::null(), |block| block.as_ptr().cast::<c_void>());
        let cwd = start.cwd.map_or(ptr::null(), <[u16]>::as_ptr);
        let mut process = PROCESS_INFORMATION::default();
        // SAFETY: the application name, the working directory and each string
        // of the environment block are NUL-terminated, the block ends with an
        // empty string, and `command_line` is NUL-terminated and writable, as
        // `CreateProcessW` may change it. `info` outlives the call. Handles
        // are inherited, those in `info` included, as std does.
        let ok = unsafe {
            CreateProcessW(
                start.application.as_ptr(),
                start.command_line.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                1,
                self.flags | CREATE_UNICODE_ENVIRONMENT,
                env,
                cwd,
                (&raw const info).cast::<STARTUPINFOW>(),
                &raw mut process,
            )
        };
        cvt(ok)?;
        // SAFETY: `CreateProcessW` returned both handles, which nothing else
        // owns. The thread's is not needed.
        let handle = unsafe {
            drop(OwnedHandle::from_raw_handle(process.hThread));
            OwnedHandle::from_raw_handle(process.hProcess)
        };
        let process = Process {
            handle,
            pid: process.dwProcessId,
        };
        Ok((process, pipes))
    }
}

/// What `CreateProcessW` gets, prepared before `SPAWN_LOCK` is taken.
struct Start<'a> {
    application: &'a [u16],
    command_line: &'a mut [u16],
    env: Option<&'a [u16]>,
    cwd: Option<&'a [u16]>,
}

impl fmt::Debug for Command {
    /// Prints the program and the arguments, as std does on Windows: quoted
    /// as `OsStr`s do, except the raw ones.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.get_program(), f)?;
        for (arg, raw) in args::tag(self.get_args(), &self.raw) {
            f.write_str(" ")?;
            if raw {
                fmt::Display::fmt(&arg.display(), f)?;
            } else {
                fmt::Debug::fmt(arg, f)?;
            }
        }
        Ok(())
    }
}

impl Stdio {
    /// Returns the inheritable handle that the child gets for the standard
    /// stream `id`, or `None` for none, and stores this process's end of a
    /// new pipe in `pipe`.
    fn to_handle(
        &self,
        id: STD_HANDLE,
        pipe: &mut Option<ChildPipe>,
    ) -> io::Result<Option<OwnedHandle>> {
        let ours_readable = id != STD_INPUT_HANDLE;
        match self {
            Self::Inherit => inherit(id),
            #[cfg(feature = "stdio")]
            Self::InheritSpecific(from) => inherit(*from),
            Self::Null => open_null(id).map(Some),
            Self::MakePipe => {
                let (ours, theirs) = pipe::pipe_pair(ours_readable)?;
                *pipe = Some(ours);
                Ok(Some(theirs))
            }
            Self::Pipe(source) => {
                pipe::spawn_relay(source, ours_readable).map(Some)
            }
            Self::Handle(handle) => {
                duplicate(handle.as_raw_handle(), true).map(Some)
            }
        }
    }
}

/// Returns an inheritable duplicate of this process's standard stream `id`,
/// or `None` if it has none, as a process without a console.
fn inherit(id: STD_HANDLE) -> io::Result<Option<OwnedHandle>> {
    // SAFETY: no preconditions.
    let handle = unsafe { GetStdHandle(id) };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Ok(None);
    }
    duplicate(handle, true).map(Some)
}

/// Opens the NUL device for the standard stream `id`, inheritable, readable
/// for stdin and writable otherwise, as std does.
fn open_null(id: STD_HANDLE) -> io::Result<OwnedHandle> {
    let access = if id == STD_INPUT_HANDLE {
        GENERIC_READ
    } else {
        GENERIC_WRITE
    };
    let security = SECURITY_ATTRIBUTES {
        // The structure is a few bytes long.
        #[allow(clippy::cast_possible_truncation)]
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: 1,
    };
    // SAFETY: the name is NUL-terminated, and `security` outlives the call.
    let handle = unsafe {
        CreateFileW(
            w!(r"\\.\NUL"),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            &raw const security,
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(null_error(id));
    }
    // SAFETY: `CreateFileW` opened a handle that nothing else owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

/// std's error for the NUL device failing to open, with its wording as a
/// static message: a failure that practically never happens links no
/// formatting code. The kind is the OS error's.
fn null_error(id: STD_HANDLE) -> io::Error {
    let kind = io::Error::last_os_error().kind();
    let message = match id {
        STD_INPUT_HANDLE => "failed to open NUL device for child stdin",
        STD_OUTPUT_HANDLE => "failed to open NUL device for child stdout",
        _ => "failed to open NUL device for child stderr",
    };
    io::Error::new(kind, message)
}

impl From<ChildPipe> for Stdio {
    fn from(pipe: ChildPipe) -> Self {
        Self::Pipe(pipe)
    }
}

/// An end of `io::pipe`, which is synchronous, so the child gets a
/// duplicate of it rather than a relay.
impl From<Pipe> for Stdio {
    fn from(pipe: Pipe) -> Self {
        Self::Handle(pipe.into_handle())
    }
}

#[cfg(feature = "fs")]
impl From<fs::File> for Stdio {
    fn from(file: fs::File) -> Self {
        Self::Handle(file.inner.into_handle())
    }
}

/// This process's standard output and error, for `From<io::Stdout>` and
/// `From<io::Stderr>`: the child gets a duplicate of the handle.
#[cfg(feature = "stdio")]
impl Stdio {
    pub(crate) const STDOUT: Self = Self::InheritSpecific(STD_OUTPUT_HANDLE);
    pub(crate) const STDERR: Self = Self::InheritSpecific(STD_ERROR_HANDLE);
}

/// A child process.
pub(crate) struct Process {
    handle: OwnedHandle,
    pid: u32,
}

impl Process {
    /// Terminates the process with exit code 1, as std does. A process that
    /// has exited already is not an error.
    pub(crate) fn kill(&mut self) -> io::Result<()> {
        // SAFETY: the handle is open.
        if unsafe { TerminateProcess(self.handle.as_raw_handle(), 1) } != 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        // A process that has exited fails with `ERROR_ACCESS_DENIED`; as in
        // std, that counts as success if the process can still be queried.
        if error.raw_os_error() == Some(os_code(ERROR_ACCESS_DENIED))
            && self.try_wait().is_ok()
        {
            return Ok(());
        }
        Err(error)
    }

    pub(crate) const fn id(&self) -> u32 {
        self.pid
    }

    /// Waits for the process to exit. Blocks the calling thread.
    pub(crate) fn wait(&mut self) -> io::Result<ExitStatus> {
        // SAFETY: the handle is open.
        let result = unsafe {
            WaitForSingleObject(self.handle.as_raw_handle(), INFINITE)
        };
        if result != WAIT_OBJECT_0 {
            return Err(io::Error::last_os_error());
        }
        self.exit_status()
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        // SAFETY: the handle is open.
        match unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) } {
            WAIT_OBJECT_0 => self.exit_status().map(Some),
            WAIT_TIMEOUT => Ok(None),
            _ => Err(io::Error::last_os_error()),
        }
    }

    /// The exit code of the process, which has exited.
    fn exit_status(&self) -> io::Result<ExitStatus> {
        let mut code = 0;
        // SAFETY: the handle is open, and `code` is writable.
        cvt(unsafe {
            GetExitCodeProcess(self.handle.as_raw_handle(), &raw mut code)
        })?;
        Ok(ExitStatus(code))
    }

    pub(crate) const fn handle(&self) -> &OwnedHandle {
        &self.handle
    }

    pub(crate) fn into_handle(self) -> OwnedHandle {
        self.handle
    }
}

/// The exit code of a process.
#[derive(PartialEq, Eq, Clone, Copy, Debug, Default)]
pub(crate) struct ExitStatus(u32);

impl ExitStatus {
    pub(crate) const fn success(self) -> bool {
        self.0 == 0
    }

    #[allow(
        clippy::unnecessary_wraps,
        reason = "`None` on Unix for a process that a signal ended"
    )]
    pub(crate) const fn code(self) -> Option<i32> {
        // std reports the bits of the `u32` code as an `i32`.
        #[allow(clippy::cast_possible_wrap)]
        let code = self.0 as i32;
        Some(code)
    }
}

impl From<u32> for ExitStatus {
    fn from(code: u32) -> Self {
        Self(code)
    }
}

impl fmt::Display for ExitStatus {
    /// Prints codes with the high bit set, which mostly mean an unhandled
    /// exception, in hex, as std does.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 & 0x8000_0000 != 0 {
            write!(f, "exit code: {:#x}", self.0)
        } else {
            write!(f, "exit code: {}", self.0)
        }
    }
}
