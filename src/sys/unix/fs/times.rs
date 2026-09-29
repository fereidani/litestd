//! Setting file times on macOS with `setattrlist`, as std does: unlike
//! `utimensat`, it sets the birth time too.

use core::ffi::{CStr, c_int};

use super::{FileTimes, cvt, timespec};
use crate::io;

/// The file whose times to set.
#[derive(Clone, Copy)]
pub(super) enum Target<'a> {
    Fd(c_int),
    /// A path, and whether a symlink there is followed or set itself.
    Path {
        path: &'a CStr,
        follow: bool,
    },
}

/// Sets the times in `times` on `target`, leaving the others unchanged.
pub(super) fn set(target: Target<'_>, times: FileTimes) -> io::Result<()> {
    let mut attrs = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: 0,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let zero = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let mut buf = [zero; 3];
    let mut len = 0;
    // The buffer holds the times that are set, in the order of their bits.
    for (time, attr) in [
        (times.created, libc::ATTR_CMN_CRTIME),
        (times.modified, libc::ATTR_CMN_MODTIME),
        (times.accessed, libc::ATTR_CMN_ACCTIME),
    ] {
        let (Some(time), Some(slot)) = (time, buf.get_mut(len)) else {
            continue;
        };
        *slot = timespec(Some(time))?;
        attrs.commonattr |= attr;
        len += 1;
    }
    let size = len * size_of::<libc::timespec>();
    let list = (&raw mut attrs).cast();
    let data = buf.as_mut_ptr().cast();
    // SAFETY: `list` describes the `len` times at `data`, `size` bytes, and
    // the calls only read both; `path` is a C string.
    let r = unsafe {
        match target {
            Target::Fd(fd) => libc::fsetattrlist(fd, list, data, size, 0),
            Target::Path { path, follow } => {
                let options = if follow { 0 } else { libc::FSOPT_NOFOLLOW };
                libc::setattrlist(path.as_ptr(), list, data, size, options)
            }
        }
    };
    cvt(r).map(drop)
}
