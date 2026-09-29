//! Cross-platform path manipulation.
//!
//! [`PathBuf`] and [`Path`] (akin to `String` and `str`) are thin wrappers
//! around [`OsString`](crate::ffi::OsString) and [`OsStr`] that parse paths
//! with the syntax of the platform. Iteration, [`Path::has_root`] and
//! comparisons ignore repeated separators, non-leading `.` components and
//! trailing separators; nothing resolves `..` components or symbolic links.

// std's methods are not `const`, so that code compiles with either crate.
#![allow(clippy::missing_const_for_fn)]

#[cfg(feature = "fs")]
mod absolute;
mod borrowed;
mod components;
mod impls;
mod owned;
mod prefix;

#[cfg(feature = "fs")]
pub use absolute::absolute;

use self::imp::{HAS_PREFIXES, is_sep_byte, is_verbatim_sep};
pub use self::{
    borrowed::{Display, Path, StripPrefixError},
    components::{Ancestors, Component, Components, Iter},
    owned::PathBuf,
    prefix::{Prefix, PrefixComponent},
};
use crate::ffi::OsStr;

/// The primary path separator: `'/'` on Unix and `'\\'` on Windows.
pub const MAIN_SEPARATOR: char = imp::MAIN_SEPARATOR;

/// [`MAIN_SEPARATOR`] as a string.
pub const MAIN_SEPARATOR_STR: &str = imp::MAIN_SEPARATOR_STR;

/// Whether `c` is a path separator on the current platform.
#[must_use]
#[inline]
pub fn is_separator(c: char) -> bool {
    u8::try_from(c).is_ok_and(is_sep_byte)
}

/// The path syntax of Windows: two separators, and prefixes.
#[cfg(windows)]
mod imp {
    pub(super) const MAIN_SEPARATOR: char = '\\';
    pub(super) const MAIN_SEPARATOR_STR: &str = "\\";
    pub(super) const HAS_PREFIXES: bool = true;

    #[inline]
    pub(super) const fn is_sep_byte(b: u8) -> bool {
        b == b'\\' || b == b'/'
    }

    /// The only separator after a verbatim prefix.
    #[inline]
    pub(super) const fn is_verbatim_sep(b: u8) -> bool {
        b == b'\\'
    }
}

/// The path syntax of Unix: one separator, and no prefixes.
#[cfg(not(windows))]
mod imp {
    pub(super) const MAIN_SEPARATOR: char = '/';
    pub(super) const MAIN_SEPARATOR_STR: &str = "/";
    pub(super) const HAS_PREFIXES: bool = false;

    #[inline]
    pub(super) const fn is_sep_byte(b: u8) -> bool {
        b == b'/'
    }

    #[inline]
    pub(super) const fn is_verbatim_sep(b: u8) -> bool {
        is_sep_byte(b)
    }
}

/// Whether `b` separates components, after a verbatim prefix or not.
#[inline]
const fn is_sep(b: u8, verbatim: bool) -> bool {
    if verbatim {
        is_verbatim_sep(b)
    } else {
        is_sep_byte(b)
    }
}

/// Wraps a piece of the encoded bytes of an `OsStr`.
///
/// # Safety
///
/// `bytes` must be a slice of one `OsStr`'s encoded bytes that starts and ends
/// at an end of that string or next to an ASCII byte.
#[inline]
unsafe fn os_str_from_piece(bytes: &[u8]) -> &OsStr {
    // SAFETY: the encoding is self-synchronizing, so such a slice may be
    // split off, as `OsStr::from_encoded_bytes_unchecked` documents.
    unsafe { OsStr::from_encoded_bytes_unchecked(bytes) }
}

/// Returns the offset in `whole` of the end of `part`, a slice of it, from
/// their addresses, which needs no unsafe code.
#[inline]
fn end_offset(whole: &[u8], part: &OsStr) -> usize {
    let part = part.as_encoded_bytes();
    part.as_ptr().addr().wrapping_sub(whole.as_ptr().addr()) + part.len()
}

/// Wraps a piece of the encoded bytes of a path in a `Path`.
///
/// # Safety
///
/// As for [`os_str_from_piece`].
#[inline]
unsafe fn path_from_piece(bytes: &[u8]) -> &Path {
    // SAFETY: guaranteed by the caller.
    Path::new(unsafe { os_str_from_piece(bytes) })
}
