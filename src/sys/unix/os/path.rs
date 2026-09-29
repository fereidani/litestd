//! Paths as the C library takes and returns them: C strings on the stack
//! unless long, and paths that the OS writes into a buffer of the caller's
//! size.

use core::ffi::CStr;

use alloc_crate::{borrow::Cow, vec::Vec};

use super::{StackBuf, name_cstr};
use crate::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
};

/// Converts `path` to a C string in `buf`, or on the heap if it does not
/// fit. A NUL byte, which would truncate the path, fails with
/// `InvalidInput`, as in std.
pub(crate) fn path_cstr<'a>(
    path: &Path,
    buf: &'a mut StackBuf,
) -> io::Result<Cow<'a, CStr>> {
    name_cstr(path.as_os_str().as_encoded_bytes(), buf)
}

/// Returns the path that `f` writes, calling it with growing buffers until
/// the path fits: 512 bytes on the stack, then doubling on the heap up to
/// 16 MiB, longer than any real path. `f` returns the path's length, or
/// `None` if the path may not have fit.
pub(crate) fn fill_path(
    f: &mut dyn FnMut(&mut [u8]) -> io::Result<Option<usize>>,
) -> io::Result<PathBuf> {
    let mut stack = [0u8; 512];
    let mut heap = Vec::new();
    for shift in 9..=24 {
        let buf: &mut [u8] = if shift == 9 {
            &mut stack
        } else {
            heap.resize(1 << shift, 0);
            &mut heap
        };
        if let Some(len) = f(buf)? {
            let path = buf.get(..len).unwrap_or_default();
            return Ok(PathBuf::from(OsString::from_unix_vec(path.to_vec())));
        }
    }
    Err(io::Error::from_raw_os_error(libc::ENAMETOOLONG))
}

/// Returns the working directory of the process.
pub(crate) fn getcwd() -> io::Result<PathBuf> {
    fill_path(&mut |buf| {
        // SAFETY: `buf` is valid for writes of its length.
        if unsafe { libc::getcwd(buf.as_mut_ptr().cast(), buf.len()) }.is_null()
        {
            let error = io::Error::last_os_error();
            return match error.raw_os_error() {
                Some(libc::ERANGE) => Ok(None),
                _ => Err(error),
            };
        }
        let path = CStr::from_bytes_until_nul(buf);
        Ok(Some(path.map_or(buf.len(), CStr::count_bytes)))
    })
}

/// Returns the target of the symbolic link `path`.
#[cfg(any(
    feature = "fs",
    all(
        feature = "env",
        any(target_os = "linux", target_os = "android", target_os = "netbsd")
    )
))]
pub(crate) fn readlink(path: &CStr) -> io::Result<PathBuf> {
    fill_path(&mut |buf| {
        // SAFETY: `path` is a C string, and `buf` is valid for writes of its
        // length, the most `readlink` writes.
        let n = super::cvt(unsafe {
            libc::readlink(path.as_ptr(), buf.as_mut_ptr().cast(), buf.len())
        })?;
        // A target that fills the buffer may have been cut short.
        let n = n.unsigned_abs();
        Ok((n < buf.len()).then_some(n))
    })
}

/// Returns the canonical, absolute form of `path`, as `realpath` resolves
/// it, free of the `PATH_MAX` limit, as in std.
#[cfg(any(feature = "fs", all(feature = "env", target_os = "openbsd")))]
pub(crate) fn realpath(path: &CStr) -> io::Result<PathBuf> {
    // SAFETY: `path` is a C string. With a null buffer, `realpath` allocates
    // the result with `malloc`.
    let resolved =
        unsafe { libc::realpath(path.as_ptr(), core::ptr::null_mut()) };
    if resolved.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `resolved` is the non-null, NUL-terminated result of
    // `realpath`, valid until the `free` below.
    let bytes = unsafe { CStr::from_ptr(resolved) }.to_bytes().to_vec();
    // SAFETY: `resolved` came from `malloc` in `realpath`, is freed once,
    // and is not used again.
    unsafe { libc::free(resolved.cast()) };
    Ok(PathBuf::from(OsString::from_unix_vec(bytes)))
}
