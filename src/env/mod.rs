//! Inspection and manipulation of the process's environment.
//!
//! The functions and iterators ending in `os` return [`OsString`]s, the
//! others [`String`]s.
//!
//! # Differences from std
//!
//! - With musl, on Android and on the BSDs, [`args()`] and [`args_os`] read the
//!   arguments from the OS when first called instead of from `argv` at startup;
//!   see [`args()`].
//! - [`set_var`] and [`remove_var`] are `unsafe` in every edition.
//! - On Unix, [`home_dir`] retries `getpwuid_r` with a larger buffer where std
//!   gives up and returns `None`.
//! - On Windows, `set_current_dir` rejects a path containing NUL, which std
//!   truncates.

mod args;
pub mod consts;
mod list;
mod paths;
mod vars;

use core::{error, fmt};

use alloc_crate::string::String;

pub use self::{
    args::{Args, ArgsOs, args, args_os},
    paths::{JoinPathsError, SplitPaths, join_paths, split_paths},
    vars::{Vars, VarsOs, vars, vars_os},
};
use crate::{
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
    sys,
};

/// Returns the current working directory as a [`PathBuf`].
///
/// This calls `getcwd` on Unix and `GetCurrentDirectoryW` on Windows.
///
/// # Errors
///
/// Fails if the current directory does not exist anymore or cannot be
/// accessed.
///
/// # Examples
///
/// ```
/// let dir = litestd::env::current_dir().unwrap();
/// assert!(dir.is_absolute());
/// ```
pub fn current_dir() -> io::Result<PathBuf> {
    sys::env::getcwd()
}

/// Changes the current working directory to the specified path.
///
/// This calls `chdir` on Unix and `SetCurrentDirectoryW` on Windows.
///
/// # Errors
///
/// Fails if the directory cannot be entered, or the path contains NUL.
pub fn set_current_dir<P: AsRef<Path>>(path: P) -> io::Result<()> {
    sys::env::chdir(path.as_ref())
}

/// Returns the full filesystem path of the current running executable.
///
/// On Linux this is the target of `/proc/self/exe`, on macOS the path that
/// `_NSGetExecutablePath` returns, which may go through symbolic links, and
/// on Windows the result of `GetModuleFileNameW`. Treat it with care in
/// privileged programs: whoever starts the process may influence it.
///
/// # Errors
///
/// Fails if the OS cannot provide the path, such as when `/proc` is not
/// mounted.
///
/// # Examples
///
/// ```
/// let exe = litestd::env::current_exe().unwrap();
/// assert!(exe.is_absolute());
/// ```
pub fn current_exe() -> io::Result<PathBuf> {
    sys::env::current_exe()
}

/// Fetches the environment variable `key` from the current process.
///
/// # Errors
///
/// Returns [`VarError::NotPresent`] if the variable is not set, or its name
/// contains `=` or NUL, and [`VarError::NotUnicode`] if its value is not
/// valid Unicode.
///
/// # Examples
///
/// ```
/// use litestd::env::{self, VarError};
///
/// let missing = env::var("LITESTD_SURELY_UNSET");
/// assert_eq!(missing, Err(VarError::NotPresent));
/// ```
pub fn var<K: AsRef<OsStr>>(key: K) -> Result<String, VarError> {
    fn inner(key: &OsStr) -> Result<String, VarError> {
        let value = sys::env::getenv(key).ok_or(VarError::NotPresent)?;
        value.into_string().map_err(VarError::NotUnicode)
    }
    inner(key.as_ref())
}

/// Fetches the environment variable `key` from the current process,
/// returning [`None`] if the variable isn't set.
///
/// It may also return `None` if the name contains `=` or NUL. The value is
/// not checked for being Unicode; [`var`] does that.
#[must_use]
pub fn var_os<K: AsRef<OsStr>>(key: K) -> Option<OsString> {
    sys::env::getenv(key.as_ref())
}

/// Sets the environment variable `key` to the value `value` for the
/// currently running process.
///
/// # Safety
///
/// Sound in a single-threaded program, and always on Windows. Elsewhere, no
/// other thread may write or read the environment meanwhile other than
/// through this module: the C library, and libraries calling it, read it
/// without synchronization, so in practice multi-threaded Unix programs
/// should not use this function at all. To pass variables to a child
/// process, set them on its command instead.
///
/// # Panics
///
/// Panics if `key` is empty or contains `=` or NUL, or if `value` contains
/// NUL.
///
/// # Examples
///
/// ```
/// use litestd::env;
///
/// // SAFETY: nothing else touches the environment in this example.
/// unsafe { env::set_var("LITESTD_EXAMPLE", "VALUE") };
/// assert_eq!(env::var("LITESTD_EXAMPLE").as_deref(), Ok("VALUE"));
/// ```
pub unsafe fn set_var<K: AsRef<OsStr>, V: AsRef<OsStr>>(key: K, value: V) {
    // SAFETY: the caller upholds the contract above, which is `setenv`'s.
    if !unsafe { sys::env::setenv(key.as_ref(), value.as_ref()) } {
        set_var_failed();
    }
}

/// Removes an environment variable from the environment of the currently
/// running process.
///
/// # Safety
///
/// As for [`set_var`].
///
/// # Panics
///
/// Panics if `key` is empty or contains `=` or NUL.
pub unsafe fn remove_var<K: AsRef<OsStr>>(key: K) {
    // SAFETY: the caller upholds the contract of `set_var`, which is
    // `unsetenv`'s.
    if !unsafe { sys::env::unsetenv(key.as_ref()) } {
        remove_var_failed();
    }
}

#[cold]
#[inline(never)]
#[track_caller]
#[allow(clippy::panic)]
fn set_var_failed() -> ! {
    // std's `set_var` panics when the OS refuses the name or value.
    panic!("failed to set environment variable")
}

#[cold]
#[inline(never)]
#[track_caller]
#[allow(clippy::panic)]
fn remove_var_failed() -> ! {
    // std's `remove_var` panics when the OS refuses the name.
    panic!("failed to remove environment variable")
}

/// Returns the path of the current user's home directory if known.
///
/// On Unix this is `HOME` unless it is empty, and otherwise the home
/// directory in the user database's entry for the current user, found with
/// `getpwuid_r`, which Android lacks. On Windows it is `USERPROFILE` unless
/// empty, and otherwise the result of `GetUserProfileDirectoryW`. On
/// WebAssembly it is `None`, as in std.
///
/// # Examples
///
/// ```
/// if let Some(home) = litestd::env::home_dir() {
///     assert!(!home.as_os_str().is_empty());
/// }
/// ```
#[must_use]
pub fn home_dir() -> Option<PathBuf> {
    sys::env::home_dir()
}

/// Returns the path of a temporary directory.
///
/// On Unix this is `TMPDIR` if set, and otherwise `/tmp`, or
/// `/data/local/tmp` on Android and on macOS the user's directory that
/// `confstr` names, if any. On Windows it is what `GetTempPath2W`
/// returns, or `GetTempPathW` before Windows 10 build 20348: the first of
/// `TMP`, `TEMP` and `USERPROFILE` that is set, or the Windows directory.
///
/// The directory may be shared with other users, so create files in it with
/// unique, unpredictable names.
#[must_use]
pub fn temp_dir() -> PathBuf {
    sys::env::temp_dir()
}

/// The error type for operations interacting with environment variables.
/// Possibly returned from [`var`].
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum VarError {
    /// The specified environment variable was not present in the current
    /// process's environment.
    NotPresent,
    /// The specified environment variable was found, but it did not contain
    /// valid Unicode data. The found data is returned as a payload of this
    /// variant.
    NotUnicode(OsString),
}

impl fmt::Display for VarError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPresent => f.write_str("environment variable not found"),
            Self::NotUnicode(s) => {
                write!(f, "environment variable was not valid unicode: {s:?}")
            }
        }
    }
}

impl error::Error for VarError {}
