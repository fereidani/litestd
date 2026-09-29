//! Operating system backends.
//!
//! Exactly one backend is compiled, selected by `target_os`: `linux` for
//! Linux and Android, `apple` for macOS 14.4 and later, `bsd` for FreeBSD,
//! NetBSD, OpenBSD and DragonFly, all on the POSIX code of `unix`,
//! `windows`, and `wasm` for WebAssembly without threads: WASI, through
//! wasi-libc and the parts of `unix` it provides, and wasm32-unknown-unknown,
//! which has no OS, where `unsupported` stands in for the OS as in std. The
//! portable layer reaches the OS only through these modules.
//! Each module compiles only for the features in parentheses; within `os`,
//! each item follows the features of its callers.
//!
//! - `futex` (`env` on Unix, `stdio`, `sync`, `thread`): `wait(&AtomicU32,
//!   u32)` sleeps while the atomic holds the value. With `sync` or `thread`,
//!   `wait_until(&AtomicU32, u32, Duration) -> bool` also gives up at an
//!   absolute deadline of `time::monotonic`, and returns `false` only once it
//!   passed. Both may return spuriously. `wake` wakes one waiter and returns
//!   `true` only if it surely did; `wake_all` wakes every waiter. A backend
//!   passes the deadline to the OS as is where it takes one on that clock, and
//!   otherwise turns it into a relative timeout, rounded up, at each call,
//!   waiting again for the time left when the OS ends the wait before the
//!   deadline.
//! - `time` (`net` on Unix, `sync`, `thread`, `time`): `monotonic() ->
//!   Duration` never goes backwards; `realtime() -> (i64, u32)` reads the wall
//!   clock as seconds, possibly negative, and nanoseconds below one billion
//!   since 1970.
//! - `os` (always): `errno`, `decode_error_kind`, `fmt_error_message` (the OS's
//!   text for a code, as std shows it), `abort`, `exit`, `id`, and, except on
//!   WebAssembly, `reference_unwinder`, which keeps the unwinder that `core`
//!   and `alloc` refer to in the link. On Unix with `io`, also `close_fd` and
//!   `duplicate_fd` for `os::fd`. It holds the helpers that the other modules
//!   share: C library results, descriptor I/O and C strings on Unix, error
//!   codes and the transfers through handles on Windows.
//! - `thread` (`thread`): `unsafe Thread::new(stack_size, NonNull<Start>)`
//!   starts a thread that calls `(start.main)(start)` once, `Start` being a
//!   `#[repr(C)]` header at the front of the caller's block. `join` waits for
//!   the thread and its thread-local destructors, dropping detaches, and
//!   `into_raw` does neither. Also `as_raw`, `set_name` (best effort),
//!   `is_main`, `sleep`, `yield_now` and `available_parallelism`.
//! - `tls` (`thread`): `usize` keys whose slots start null;
//!   `create_with_hook::<H>()` passes an exiting thread's non-null values to
//!   `H::run`. `get`, `set` (`false` when out of memory) and `destroy` are
//!   unsafe. With `nightly` on Apple targets, `tlv::at_exit(f)` makes the
//!   calling thread run `f` as it exits, before dyld frees its native
//!   thread-locals, which is too late for the key hooks.
//! - `alloc` (always): `alloc`, `alloc_zeroed`, `dealloc` and `realloc` with
//!   the `GlobalAlloc` contracts, for every `Layout`.
//! - `stdio` (`stdio`, `panic-location`): `write(Stream, &[u8])` writes once to
//!   `STDOUT` or `STDERR` (Windows consoles as UTF-16, carrying a split
//!   character to the next write, which `reset` forgets), returning the count
//!   or the OS error code, which `is_interrupted` and `is_ebadf` (a closed or
//!   missing stream) classify; `thread_id()` is unique among live threads and
//!   never 0. With `io`, `write_error` turns a code into std's error, and
//!   `Stdin::read` (Windows consoles as UTF-16), `STDIN` and `is_terminal`.
//!   Windows transfers wait for overlapped handles and abort if one stays in
//!   flight.
//! - `terminal` (`io`, not on wasm32-unknown-unknown): `is_terminal` of a raw
//!   descriptor or handle, with std's msys and cygwin heuristic on Windows.
//! - `fs` (`fs`): std's internal `sys::fs` API on `&Path`. A path the OS cannot
//!   represent fails with `InvalidInput`, never truncated; invalid
//!   `OpenOptions` fail as in std; `ReadDir` skips `.` and `..`;
//!   `remove_dir_all` never follows symlinks, even racing ones.
//! - `pipe` (`io`): `pipe()` returns the `(reader, writer)` ends of an
//!   anonymous pipe as `Pipe`s, which children do not inherit, synchronous
//!   handles on Windows. `Pipe` reads and writes through `&self`, reads to the
//!   end without zeroing the vector, has `try_clone`, std's `Debug`, and
//!   converts from and to `OwnedFd` or `OwnedHandle`. A read after every writer
//!   is gone returns 0. The module also creates the pipes to children.
//! - `process` (`command`): std's internal `sys::process` API. `Command` keeps
//!   the program and arguments in the portable `process::args::Args` and the
//!   environment changes in `process::env::CommandEnv`, keyed by `EnvKey`
//!   (`Ord` in the platform's order, `From<&OsStr>`, `AsRef<OsStr>`,
//!   `PartialEq<str>`). It has `new`, `arg`, `env_mut`, `cwd`, `stdin`,
//!   `stdout`, `stderr`, `get_program`, `get_args`, `get_envs`,
//!   `get_current_dir`, std's `Debug`, and `spawn(default, needs_stdin)`, which
//!   gives the unset streams `default` (stdin `Null` unless `needs_stdin`),
//!   returns a `Process` and the parent's pipe ends as `StdioPipes`, and fails
//!   with `InvalidInput` on a NUL byte before creating anything. `Stdio`:
//!   `Inherit`, `Null`, `MakePipe`, and `From` a `ChildPipe`, a `pipe::Pipe`
//!   or, with `fs`, a `fs::File`. `ChildPipe` reads and writes through `&self`
//!   and reads to the end without zeroing; `read_output` drains two pipes at
//!   once. `Process`: `id`, `kill` (`Ok` without a signal once reaped), and
//!   `wait` and `try_wait`, which keep the status. `ExitStatus`: `Copy`, `Eq`,
//!   `Default` (success), std's `Debug` and `Display`, `success`, `code`. No
//!   handle created for a child leaks into another.
//! - `net` (`net`): std's internal `sys::net` API. Child processes do not
//!   inherit sockets, zero timeouts fail with `net::ZERO_TIMEOUT`, a read after
//!   a read shutdown returns 0, streams read to the end without zeroing the
//!   vector, writes on a closed connection fail with `BrokenPipe` instead of
//!   raising a signal, and `lookup_host` rejects a NUL byte with std's error.
//! - `env` (`env`): `args()` and `vars()` return the arguments, as a
//!   `Cow<'static, [Unit]>`, and a snapshot of the `KEY=VALUE` variables, as a
//!   `Vec<Unit>` (bytes on Unix, UTF-16 on Windows), each string followed by a
//!   NUL, with their count; after `args()`, `args_text()` gives the same units
//!   as one `str` if they are shared and UTF-8. `find_nul` searches such units,
//!   and `os_string` converts one string. `getenv`, `unsafe setenv` and `unsafe
//!   unsetenv` (`false` when refused), `temp_dir`, `home_dir`, `getcwd`,
//!   `chdir` and `current_exe`. On Unix, `EnvGuard::read()` holds the lock that
//!   orders every reader of the environment before or after `setenv`.
//!
//! macOS cannot create a pipe or socket close-on-exec in one call, so there,
//! as in std, a child that another thread spawns meanwhile can inherit one
//! being created.

#[cfg(target_os = "macos")]
mod apple;
#[cfg(any(
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
mod bsd;
#[cfg(any(target_os = "linux", target_os = "android"))]
mod linux;
#[cfg(any(unix, target_os = "wasi"))]
mod unix;
// The services that a wasm module lacks, which only I/O can ask for.
#[cfg(all(
    target_family = "wasm",
    any(target_os = "wasi", target_os = "unknown"),
    feature = "io"
))]
mod unsupported;
#[cfg(all(
    target_family = "wasm",
    any(target_os = "wasi", target_os = "unknown")
))]
mod wasm;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub(crate) use apple::*;
#[cfg(any(
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
pub(crate) use bsd::*;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) use linux::*;
#[cfg(all(
    target_family = "wasm",
    any(target_os = "wasi", target_os = "unknown")
))]
pub(crate) use wasm::*;
#[cfg(windows)]
pub(crate) use windows::*;

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    windows,
    all(
        target_family = "wasm",
        any(target_os = "wasi", target_os = "unknown")
    )
)))]
compile_error!("litestd does not support this target yet");

// Threads block on `memory.atomic.wait32`, whose intrinsic is unstable.
#[cfg(all(
    target_family = "wasm",
    target_feature = "atomics",
    not(feature = "nightly")
))]
compile_error!(
    "litestd needs its `nightly` feature for WebAssembly with threads (the \
     `atomics` target feature): Rust's `memory.atomic.wait32` is unstable"
);
