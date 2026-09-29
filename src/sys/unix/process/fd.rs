//! The descriptors of a child's standard streams that are not pipes, and
//! reading two pipes from a child at once.

use alloc_crate::vec::Vec;

use crate::{
    io::{self, UninitReadToEnd},
    os::fd::{FromRawFd, OwnedFd},
    sys::{os::cvt_r, pipe::Pipe},
};

/// Opens `/dev/null` close-on-exec, write-only if `write` is set and
/// read-only otherwise, so that a child can never modify it.
pub(crate) fn open_null(write: bool) -> io::Result<Pipe> {
    let access = if write {
        libc::O_WRONLY
    } else {
        libc::O_RDONLY
    };
    let flags = access | libc::O_CLOEXEC;
    // SAFETY: the path is a C string; without `O_CREAT`, no mode is read.
    let fd = cvt_r(|| unsafe { libc::open(c"/dev/null".as_ptr(), flags) })?;
    // SAFETY: `open` returned a new descriptor that nothing else owns.
    Ok(Pipe::from(unsafe { OwnedFd::from_raw_fd(fd) }))
}

/// Duplicates this process's standard stream `fd`, as std does for a child
/// given `io::stdout()` or `io::stderr()`.
#[cfg(feature = "stdio")]
pub(crate) fn duplicate_stdio(fd: core::ffi::c_int) -> io::Result<Pipe> {
    debug_assert!((0..=2).contains(&fd), "a standard stream");
    // SAFETY: the borrow lasts for the duplication alone, and the standard
    // streams stay open, as std assumes too; a closed one fails with EBADF.
    let fd = unsafe { crate::os::fd::BorrowedFd::borrow_raw(fd) };
    fd.try_clone_to_owned().map(Pipe::from)
}

/// Reads two pipes to their ends at once, so that a child blocked writing
/// one cannot deadlock a parent blocked reading the other.
///
/// Unlike std, it leaves the pipes blocking: after `poll` reports a pipe
/// ready, it holds data or is closed, so one read never blocks. That trades
/// std's mode switches and its reads that only report `EAGAIN` for a `poll`
/// per read: fewer calls for short output, more for a steady stream.
#[allow(clippy::needless_pass_by_value, reason = "closes the pipes")]
pub(crate) fn read_output(
    out: Pipe,
    stdout: &mut Vec<u8>,
    err: Pipe,
    stderr: &mut Vec<u8>,
) -> io::Result<()> {
    let pollfd = |pipe: &Pipe| libc::pollfd {
        fd: pipe.raw(),
        events: libc::POLLIN,
        revents: 0,
    };
    let mut fds = [pollfd(&out), pollfd(&err)];
    let mut stdout = UninitReadToEnd::new(stdout);
    let mut stderr = UninitReadToEnd::new(stderr);
    // Blocks until a pipe is ready; each pass reads at least one byte, or
    // finds an end and drains the other pipe. The child's exit closes both.
    loop {
        // SAFETY: `fds` is valid for reads and writes of two `pollfd`s.
        cvt_r(|| unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) })?;
        if fds[0].revents != 0 && read_once(&out, &mut stdout)? {
            return drain(&err, &mut stderr);
        }
        if fds[1].revents != 0 && read_once(&err, &mut stderr)? {
            return drain(&out, &mut stdout);
        }
    }
}

/// Reads once from a pipe, returning whether it reached its end.
fn read_once(pipe: &Pipe, buf: &mut UninitReadToEnd<'_>) -> io::Result<bool> {
    // SAFETY: `read_uninit` initializes what it reports, nothing else.
    let end = unsafe { buf.read_once(|spare| pipe.read_uninit(spare)) }?;
    Ok(end.is_some())
}

/// Reads the rest of a pipe, blocking, as the other pipe has ended.
fn drain(pipe: &Pipe, buf: &mut UninitReadToEnd<'_>) -> io::Result<()> {
    // Each pass reads at least one byte or retries after a signal; the pipe
    // ends once the child exits, if not before.
    while !read_once(pipe, buf)? {}
    Ok(())
}
