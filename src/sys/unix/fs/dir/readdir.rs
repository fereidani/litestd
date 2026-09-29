//! Directory entries from `readdir`, as std reads them on macOS, which has
//! no public call that reads raw records. The stream comes from `fdopendir`
//! on the descriptor that `openat` opened.

use core::{ffi::CStr, ptr::NonNull};

use alloc_crate::vec::Vec;

use super::{valid_name, without_nul};
use crate::{
    io,
    os::fd::{AsRawFd, IntoRawFd, OwnedFd, RawFd},
    sys::os,
};

/// An open directory stream, which owns its descriptor and closes both when
/// dropped.
pub(super) struct Handle {
    dir: NonNull<libc::DIR>,
    /// The stream's descriptor, kept so that reading it never touches the
    /// stream.
    fd: RawFd,
}

// SAFETY: a stream may be used from any thread, one at a time, which the
// contract of `Records::advance` asks of its readers.
unsafe impl Send for Handle {}
// SAFETY: a shared `Handle` only gives out its descriptor and the stream
// for `Records::advance`, whose callers guarantee that one reads it at a
// time.
unsafe impl Sync for Handle {}

/// Opens the directory stream of `fd`, a descriptor of the directory.
pub(super) fn open(fd: OwnedFd) -> io::Result<Handle> {
    // SAFETY: `fd` is an open descriptor, which the stream takes over on
    // success.
    let dir = unsafe { libc::fdopendir(fd.as_raw_fd()) };
    let dir = NonNull::new(dir).ok_or_else(io::Error::last_os_error)?;
    Ok(Handle {
        dir,
        fd: fd.into_raw_fd(),
    })
}

impl Handle {
    /// Starts the stream over from the first entry.
    ///
    /// # Safety
    ///
    /// No `Records` may read the stream at the same time.
    #[cfg(target_os = "wasi")]
    pub(super) unsafe fn rewind(&self) {
        // SAFETY: the stream is open, and the caller rules out readers.
        unsafe { libc::rewinddir(self.dir.as_ptr()) };
    }
}

impl AsRawFd for Handle {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the stream is open and not used again.
        let r = unsafe { libc::closedir(self.dir.as_ptr()) };
        // It fails only as `close` does, which frees the descriptor anyway.
        debug_assert!(r == 0 || os::errno() != libc::EBADF);
    }
}

/// The entry of a stream that `advance` moved to last, copied out of the
/// stream, whose memory the next `readdir` reuses.
pub(super) struct Records {
    /// The name, NUL included; empty before the first entry.
    name: Vec<u8>,
    ino: u64,
    kind: u8,
}

impl Records {
    pub(super) const fn new() -> Self {
        Self {
            name: Vec::new(),
            ino: 0,
            kind: 0,
        }
    }

    /// Records that hold no memory.
    pub(super) const fn empty() -> Self {
        Self::new()
    }

    /// Moves to the next entry of `dir` other than `.` and `..`. Returns
    /// `false` at the end of the directory.
    ///
    /// # Safety
    ///
    /// No other `Records` may read `dir` at the same time: `readdir` changes
    /// the stream without a lock.
    pub(super) unsafe fn advance(&mut self, dir: &Handle) -> io::Result<bool> {
        // Each pass consumes an entry; the stream ends by returning null.
        loop {
            // `readdir` leaves `errno` alone at the end of the stream, so a
            // cleared `errno` tells the end from an error, as in std.
            // SAFETY: `errno_location` points to the thread's `errno`.
            unsafe { os::errno_location().write(0) };
            // SAFETY: the stream is open, and the caller rules out other
            // readers.
            let entry = unsafe { libc::readdir(dir.dir.as_ptr()) };
            let Some(entry) = NonNull::new(entry) else {
                return match os::errno() {
                    0 => Ok(false),
                    code => Err(io::Error::from_raw_os_error(code)),
                };
            };
            let entry = entry.as_ptr();
            // SAFETY: `readdir` returned an entry, valid until the next call
            // on the stream, whose name is a C string. The fields are read
            // through the pointer alone: the entry may be shorter than the
            // declared `dirent`, so no reference to one is made.
            let (name, ino, kind) = unsafe {
                // The BSDs name the inode field `d_fileno`.
                #[cfg(any(target_vendor = "apple", target_os = "wasi"))]
                let ino = (&raw const (*entry).d_ino).read_unaligned();
                #[cfg(not(any(target_vendor = "apple", target_os = "wasi")))]
                let ino = (&raw const (*entry).d_fileno).read_unaligned();
                (
                    CStr::from_ptr((&raw const (*entry).d_name).cast()),
                    ino,
                    (&raw const (*entry).d_type).read(),
                )
            };
            let name = name.to_bytes_with_nul();
            let bare = without_nul(name);
            if bare == b"." || bare == b".." {
                continue;
            }
            if !valid_name(bare) {
                return Err(super::MALFORMED);
            }
            self.name.clear();
            self.name.extend_from_slice(name);
            self.ino = ino;
            self.kind = kind;
            return Ok(true);
        }
    }

    /// The name of the current entry, NUL included.
    pub(super) fn name_with_nul(&self) -> &[u8] {
        &self.name
    }

    pub(super) const fn ino(&self) -> u64 {
        self.ino
    }

    /// The `d_type` of the current entry.
    pub(super) const fn kind(&self) -> u8 {
        self.kind
    }
}
