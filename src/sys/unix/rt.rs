//! The startup work of std's runtime, which litestd does from an
//! initializer that the loader runs before `main`: it opens `/dev/null` on
//! the standard descriptors that are closed and ignores `SIGPIPE`, and with
//! glibc it keeps the arguments for `env::args`. A shared library built on
//! litestd does the same in the process that loads it.

#[cfg(all(target_os = "linux", target_env = "gnu"))]
use core::ffi::{c_char, c_int};

use super::os;

/// The initializer. glibc passes it `argc`, `argv` and `envp`, as std
/// relies on; other C libraries and loaders may pass nothing, so it takes
/// nothing there.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[used]
#[unsafe(link_section = ".init_array")]
static INIT: extern "C" fn(c_int, *const *const c_char, *const *const c_char) =
    init;

/// As above, without arguments.
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
#[used]
#[cfg_attr(not(target_vendor = "apple"), unsafe(link_section = ".init_array"))]
#[cfg_attr(
    target_vendor = "apple",
    unsafe(link_section = "__DATA,__mod_init_func")
)]
static INIT: extern "C" fn() = init;

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[cfg_attr(not(feature = "env"), allow(unused_variables))]
#[allow(clippy::similar_names, reason = "C's names")]
extern "C" fn init(
    argc: c_int,
    argv: *const *const c_char,
    _envp: *const *const c_char,
) {
    setup();
    #[cfg(feature = "env")]
    super::env::save_args(argc, argv);
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
extern "C" fn init() {
    setup();
}

/// Opens `/dev/null` on each standard descriptor that is closed, as std
/// does, so that no file the program opens later takes its place and
/// receives what the program prints; aborts if that fails, as std does.
/// Then ignores `SIGPIPE`, as std does, so that a write to a pipe or socket
/// without a reader fails with `BrokenPipe` instead of ending the process.
fn setup() {
    for fd in 0..3 {
        // SAFETY: `F_GETFD` reads the descriptor's flags; no memory is
        // involved.
        let closed = unsafe { libc::fcntl(fd, libc::F_GETFD) } == -1
            && os::errno() == libc::EBADF;
        if !closed {
            continue;
        }
        // `open` returns the lowest free descriptor, `fd`, as those below it
        // are open. Children inherit it, as their standard stream.
        // SAFETY: the path is a C string.
        let r = unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDWR) };
        if r == -1 {
            os::abort();
        }
    }
    // SAFETY: the disposition of `SIGPIPE` involves no memory.
    let previous = unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
    debug_assert_ne!(previous, libc::SIG_ERR);
}
