//! The environment of a module without an OS, as std has it there: no
//! arguments or variables, no directories, and a list of variables that
//! panics.

use alloc_crate::{borrow::Cow, vec::Vec};

use super::UNSUPPORTED;
use crate::{
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
};

/// A unit of the strings the OS would hand out.
pub(crate) type Unit = u8;

/// Copies units into an `OsString`.
pub(crate) fn os_string(units: &[u8]) -> OsString {
    OsString::from_unix_vec(units.to_vec())
}

/// The index of the first NUL in `units`.
pub(crate) fn find_nul(units: &[u8]) -> Option<usize> {
    units.iter().position(|&unit| unit == 0)
}

/// No arguments.
pub(crate) fn args() -> (Cow<'static, [u8]>, usize) {
    (Cow::Borrowed(&[]), 0)
}

/// The arguments as text, which there are none of.
pub(crate) fn args_text() -> Option<&'static str> {
    Some("")
}

/// Panics as std does: there is no environment to list.
#[cold]
#[track_caller]
#[allow(clippy::panic, reason = "std panics here")]
pub(crate) fn vars() -> (Vec<u8>, usize) {
    panic!("not supported on this platform")
}

pub(crate) fn getenv(_key: &OsStr) -> Option<OsString> {
    None
}

/// Refuses, as std does there.
///
/// # Safety
///
/// None; the signature matches the other backends.
pub(crate) unsafe fn setenv(_key: &OsStr, _value: &OsStr) -> bool {
    false
}

/// Refuses, as std does there.
///
/// # Safety
///
/// None; the signature matches the other backends.
pub(crate) unsafe fn unsetenv(_key: &OsStr) -> bool {
    false
}

/// Panics as std does: there is no file system.
#[cold]
#[track_caller]
#[allow(clippy::panic, reason = "std panics here")]
pub(crate) fn temp_dir() -> PathBuf {
    panic!("no filesystem on this platform")
}

pub(crate) fn home_dir() -> Option<PathBuf> {
    None
}

pub(crate) fn getcwd() -> io::Result<PathBuf> {
    Err(UNSUPPORTED)
}

pub(crate) fn chdir(_path: &Path) -> io::Result<()> {
    Err(UNSUPPORTED)
}

pub(crate) fn current_exe() -> io::Result<PathBuf> {
    Err(UNSUPPORTED)
}
