//! Paths as the Windows API takes them: NUL-terminated UTF-16, made verbatim
//! (`\\?\`) where std does so that `MAX_PATH` does not limit them.

use core::ptr;

use alloc_crate::vec::Vec;
use windows_sys::Win32::Storage::FileSystem::GetFullPathNameW;

#[cfg(feature = "fs")]
use super::os::wide::NUL_ERROR;
pub(crate) use super::os::wide::{WideBuf, to_path_buf};
use super::os::wide::{assume_init, to_wide};
#[cfg(feature = "fs")]
use crate::path::PathBuf;
use crate::{ffi::OsStr, io};

/// Units reserved in front of `GetFullPathNameW`'s output for the longest
/// verbatim prefix, `\\?\UNC\`.
const PREFIX_UNITS: usize = 8;

pub(crate) const SEP: u16 = b'\\' as u16;
const ALT_SEP: u16 = b'/' as u16;
pub(crate) const COLON: u16 = b':' as u16;
pub(crate) const QUERY: u16 = b'?' as u16;
pub(crate) const DOT: u16 = b'.' as u16;

/// The verbatim prefix, `\\?\`.
pub(crate) const VERBATIM: [u16; 4] = [SEP, SEP, QUERY, SEP];
/// The NT object namespace prefix, `\??\`.
pub(crate) const NT_PREFIX: [u16; 4] = [SEP, QUERY, QUERY, SEP];
/// The verbatim prefix for UNC paths, `\\?\UNC\`.
pub(crate) const VERBATIM_UNC: [u16; 8] = [
    SEP,
    SEP,
    QUERY,
    SEP,
    b'U' as u16,
    b'N' as u16,
    b'C' as u16,
    SEP,
];

/// Converts `s` to NUL-terminated UTF-16 without making it verbatim, as
/// std's `to_u16s` does.
pub(crate) fn to_u16s<'a>(
    buf: &'a mut WideBuf,
    s: &OsStr,
) -> io::Result<&'a [u16]> {
    to_wide(buf, s, &[])
}

/// Returns whether `wide` is already in its final form: verbatim, empty, or
/// absolute and short enough for every Windows function.
pub(crate) fn keeps_form(wide: &[u16]) -> bool {
    /// `MAX_PATH` minus the room some functions need for a file name: the
    /// length from which std makes even absolute paths verbatim.
    const LEGACY_MAX_PATH: usize = 248;

    wide.starts_with(&VERBATIM)
        || wide.starts_with(&NT_PREFIX)
        || wide == [0]
        || (wide.len() < LEGACY_MAX_PATH
            && match *wide {
                // `D:` alone, or `D:` followed by a separator.
                [drive, COLON, 0] | [drive, COLON, SEP | ALT_SEP, ..] => {
                    drive != SEP && drive != ALT_SEP
                }
                // UNC and device paths.
                [SEP | ALT_SEP, SEP | ALT_SEP, ..] => true,
                _ => false,
            })
}

/// A path converted the way std's `maybe_verbatim` converts it.
pub(crate) struct NativePath {
    input: WideBuf,
    full: WideBuf,
}

impl NativePath {
    /// Creates the buffers, without initializing or allocating anything.
    pub(crate) const fn new() -> Self {
        Self {
            input: WideBuf::new(),
            full: WideBuf::new(),
        }
    }

    /// Converts `path` for the Windows API and returns it with its NUL.
    /// Verbatim and short absolute paths are only re-encoded; others are made
    /// absolute and normalized by `GetFullPathNameW`, then verbatim.
    pub(crate) fn convert(&mut self, path: &OsStr) -> io::Result<&[u16]> {
        self.convert_with(path, &[])
    }

    /// Like [`Self::convert`], for `path` followed by `suffix`.
    pub(crate) fn convert_with(
        &mut self,
        path: &OsStr,
        suffix: &[u16],
    ) -> io::Result<&[u16]> {
        let wide = to_wide(&mut self.input, path, suffix)?;
        if keeps_form(wide) {
            return Ok(wide);
        }
        let input = wide.as_ptr();
        let (space, len) = self.full.fill(PREFIX_UNITS, &mut |buf, size| {
            // SAFETY: `input` is NUL-terminated and outlives this call, and
            // `buf` is valid for writes of `size` units.
            unsafe { GetFullPathNameW(input, size, buf, ptr::null_mut()) }
        })?;
        let end = PREFIX_UNITS + len + 1;
        let written = space.get(PREFIX_UNITS..end).unwrap_or_default();
        // SAFETY: `fill` returned the length of a successful call, below the
        // length of the space that it zeroed from `PREFIX_UNITS` on, so the
        // units that `written` covers, the NUL included, are initialized.
        let absolute = unsafe { assume_init(written) };
        // The path is now absolute and normalized, so separators are `\`.
        let (skip, prefix): (usize, &[u16]) = match *absolute {
            [_, COLON, SEP, ..] => (0, &VERBATIM),
            [SEP, SEP, DOT, SEP, ..] => (4, &VERBATIM),
            // Already verbatim, or in the NT namespace.
            [SEP, SEP | QUERY, QUERY, SEP, ..] => (0, &[]),
            [SEP, SEP, ..] => (2, &VERBATIM_UNC),
            _ => (0, &[]),
        };
        // The prefix replaces the `skip` units in front of the rest of the
        // path, and extends into the room reserved before it.
        let start = PREFIX_UNITS + skip - prefix.len();
        for (slot, &unit) in space.iter_mut().skip(start).zip(prefix) {
            slot.write(unit);
        }
        let native = space.get(start..end).unwrap_or_default();
        // SAFETY: the loop wrote the units from `start` to at least
        // `PREFIX_UNITS`, and `fill` zeroed those from `PREFIX_UNITS` to `end`
        // before the call that filled them.
        Ok(unsafe { assume_init(native) })
    }
}

/// Returns the full path of the NUL-terminated `wide` in `buf`, without its
/// NUL: absolute and normalized, as `GetFullPathNameW` makes it.
pub(crate) fn full_path<'a>(
    buf: &'a mut WideBuf,
    wide: &[u16],
) -> io::Result<&'a [u16]> {
    debug_assert_eq!(wide.last(), Some(&0));
    buf.fill_wide(&mut |out, size| {
        // SAFETY: `wide` is NUL-terminated and outlives this call, and `out`
        // is valid for writes of `size` units.
        unsafe { GetFullPathNameW(wide.as_ptr(), size, out, ptr::null_mut()) }
    })
}

/// Whether the NUL-terminated `wide` is absolute and normalized, which
/// `GetFullPathNameW` leaves as it is.
pub(crate) fn is_full_path(wide: &[u16]) -> io::Result<bool> {
    let mut buf = WideBuf::new();
    let full = full_path(&mut buf, wide)?;
    Ok(Some(full) == wide.split_last().map(|(_, path)| path))
}

/// Gives the NUL-terminated `wide` the form of std's
/// `from_wide_to_user_path` where that applies, since `cmd.exe` and working
/// directories do not take verbatim paths: a verbatim drive path,
/// `\\?\C:\...`, or UNC path, `\\?\UNC\...`, loses its prefix if the rest
/// means the same without it, and a path over `MAX_PATH` units stays as it
/// is. Returns `false` for any other path, which the caller must still make
/// absolute.
pub(crate) fn user_form(wide: &mut Vec<u16>) -> io::Result<bool> {
    /// `MAX_PATH`, NUL included; std leaves longer paths verbatim.
    const MAX_PATH: usize = 260;

    if wide.len() > MAX_PATH {
        return Ok(true);
    }
    // Where the path would start without its verbatim prefix.
    let start = match wide.as_slice() {
        [SEP, SEP, QUERY, SEP, _, COLON, SEP, ..] => 4,
        _ if wide.starts_with(&VERBATIM_UNC) => {
            // `\\?\UNC\server` becomes `\\server`.
            if let Some(unit) = wide.get_mut(6) {
                *unit = SEP;
            }
            6
        }
        _ => return Ok(false),
    };
    if is_full_path(wide.get(start..).unwrap_or_default())? {
        wide.drain(..start);
    } else if let (6, Some(unit)) = (start, wide.get_mut(6)) {
        *unit = u16::from(b'C');
    }
    Ok(true)
}

/// Makes `path` absolute as std's `path::absolute` does on Windows: verbatim
/// paths stay unchanged, others go through `GetFullPathNameW`.
#[cfg(feature = "fs")]
pub(crate) fn absolute(path: &OsStr) -> io::Result<PathBuf> {
    let bytes = path.as_encoded_bytes();
    if bytes.starts_with(br"\\?\") {
        // Rejected for consistency with the other paths.
        if bytes.contains(&0) {
            return Err(NUL_ERROR);
        }
        return Ok(PathBuf::from(path));
    }
    let mut input = WideBuf::new();
    let wide = to_u16s(&mut input, path)?;
    full_path(&mut WideBuf::new(), wide).map(to_path_buf)
}
