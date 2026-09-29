//! `copy` on macOS, as std does it: `fclonefileat`, which clones the file on
//! APFS, sharing its blocks and copying its metadata, and `fcopyfile` where
//! no clone can be made.

use core::ffi::c_int;

use super::{
    FileType, STACK_BUF, attr::fstat, cstr, cvt, cvt_r, open_c, to_mode,
};
use crate::{io, os::fd::AsRawFd, path::Path};

pub(crate) fn copy(from: &Path, to: &Path) -> io::Result<u64> {
    let (mut buf_from, mut buf_to) = (STACK_BUF, STACK_BUF);
    let from = cstr(from, &mut buf_from)?;
    let reader = open_c(&from, libc::O_RDONLY, 0)?;
    let attr = fstat(reader.as_raw_fd())?;
    if !attr.file_type().is_file() {
        return Err(io::const_error!(
            io::ErrorKind::InvalidInput,
            "the source path is neither a regular file nor a symlink to a \
             regular file",
        ));
    }
    let to = cstr(to, &mut buf_to)?;
    // SAFETY: `to` is a C string, which the call only reads.
    let cloned = cvt(unsafe {
        libc::fclonefileat(reader.as_raw_fd(), libc::AT_FDCWD, to.as_ptr(), 0)
    });
    match cloned {
        Ok(_) => return Ok(attr.size()),
        // Another file system than APFS, an existing destination, or
        // another device: `fcopyfile` copies then.
        Err(e)
            if matches!(
                e.raw_os_error(),
                Some(libc::ENOTSUP | libc::EEXIST | libc::EXDEV)
            ) => {}
        Err(e) => return Err(e),
    }
    let mode = to_mode(attr.perm().mode());
    let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC;
    let writer = open_c(&to, flags, mode)?;
    let sink = fstat(writer.as_raw_fd())?.file_type();
    // A new file gets the mode minus the umask, and an existing one keeps
    // its permissions: set them exactly, but on regular files only, so that
    // copying to a device or a FIFO never changes its permissions.
    if sink.is_file() {
        let fd = writer.as_raw_fd();
        // SAFETY: `fchmod` touches no memory.
        cvt_r(|| unsafe { libc::fchmod(fd, mode) })?;
    }
    copy_data(reader.as_raw_fd(), writer.as_raw_fd(), sink)
}

/// Copies `reader` into `writer`, a file of type `sink`, with `fcopyfile`:
/// its data, and also its metadata, extended attributes and ACLs if `sink`
/// is a regular file. Returns the number of bytes copied.
fn copy_data(reader: c_int, writer: c_int, sink: FileType) -> io::Result<u64> {
    let flags = if sink.is_file() {
        libc::COPYFILE_METADATA | libc::COPYFILE_DATA
    } else {
        libc::COPYFILE_DATA
    };
    let state = State::new()?;
    // SAFETY: both descriptors are open, and `state` is a live state.
    cvt(unsafe { libc::fcopyfile(reader, writer, state.0, flags) })?;
    let mut copied: libc::off_t = 0;
    // SAFETY: `COPYFILE_STATE_COPIED` stores an `off_t` through the pointer.
    cvt(unsafe {
        libc::copyfile_state_get(
            state.0,
            libc::COPYFILE_STATE_COPIED.unsigned_abs(),
            (&raw mut copied).cast(),
        )
    })?;
    Ok(copied.unsigned_abs())
}

/// A `copyfile` state, freed when dropped. It is never null.
struct State(libc::copyfile_state_t);

impl State {
    fn new() -> io::Result<Self> {
        // SAFETY: `copyfile_state_alloc` has no preconditions.
        let state = unsafe { libc::copyfile_state_alloc() };
        if state.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(state))
    }
}

impl Drop for State {
    fn drop(&mut self) {
        // SAFETY: the state is live and not used again.
        let r = unsafe { libc::copyfile_state_free(self.0) };
        // It fails only if closing files that `copyfile` opened fails, and
        // `fcopyfile` opens none.
        debug_assert_eq!(r, 0);
    }
}
