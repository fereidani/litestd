//! The process environment on the Windows API: the command line, parsed as
//! std parses it, the environment block and variables, and the current,
//! temporary and home directories.

use core::{iter, mem, ptr, slice};

use alloc_crate::{borrow::Cow, vec::Vec};
use windows_sys::{
    Win32::{
        Foundation::{ERROR_INSUFFICIENT_BUFFER, HANDLE},
        Storage::FileSystem::GetTempPathW,
        System::{
            Environment::{
                GetCommandLineW, GetCurrentDirectoryW, GetEnvironmentVariableW,
                SetCurrentDirectoryW, SetEnvironmentVariableW,
            },
            LibraryLoader::{
                GetModuleFileNameW, GetModuleHandleW, GetProcAddress,
            },
        },
        UI::Shell::GetUserProfileDirectoryW,
    },
    core::w,
};

use super::os::{
    cvt,
    environ::{EnvironmentStrings, push_var},
    last_error,
    wide::{WideBuf, fill_os_string, to_wide},
};
use crate::{
    ffi::{OsStr, OsString},
    io,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};

/// A unit of the strings the OS hands out.
pub(crate) type Unit = u16;

const TAB: u16 = b'\t' as u16;
const SPACE: u16 = b' ' as u16;
const QUOTE: u16 = b'"' as u16;
const BACKSLASH: u16 = b'\\' as u16;

/// Copies units the OS handed out into an `OsString`.
pub(crate) fn os_string(units: &[u16]) -> OsString {
    OsString::from_wtf16(units)
}

/// The index of the first NUL in `units`.
pub(crate) fn find_nul(units: &[u16]) -> Option<usize> {
    units.iter().position(|&unit| unit == 0)
}

/// The path of the executable.
fn exe_path() -> io::Result<OsString> {
    fill_os_string(&mut |buf, size| {
        // SAFETY: `buf` is valid for writes of `size` units; a null module
        // names the executable.
        unsafe { GetModuleFileNameW(ptr::null_mut(), buf, size) }
    })
}

/// Parses the command line as std does, into the arguments, each followed
/// by a NUL, and their count.
pub(crate) fn args() -> (Cow<'static, [u16]>, usize) {
    // SAFETY: `GetCommandLineW` has no preconditions and returns the
    // process's command line, which lives as long as the process.
    let line = unsafe { command_line(GetCommandLineW()) };
    // Every argument but the last ends at a blank that its NUL replaces, and
    // no argument is longer than its text, so this never reallocates.
    let mut out = Vec::with_capacity(line.len() + 1);
    let count = if line.is_empty() {
        // Without a command line, std's only argument names the executable.
        if let Ok(exe) = exe_path() {
            out.extend(exe.encode_wide());
        }
        out.push(0);
        1
    } else {
        parse_command_line(line, &mut out)
    };
    (Cow::Owned(out), count)
}

/// The arguments as one `str`: never, as no `str` views UTF-16.
pub(crate) const fn args_text() -> Option<&'static str> {
    None
}

/// The units of the command line at `line`, up to its NUL.
///
/// # Safety
///
/// `line` must be null or a NUL-terminated string that lives as long as the
/// process, and that no thread writes.
const unsafe fn command_line(line: *const u16) -> &'static [u16] {
    /// The longest command line Windows accepts, in units.
    const MAX_LEN: usize = 32767;
    if line.is_null() {
        return &[];
    }
    let mut len = 0;
    // SAFETY: the string is NUL-terminated and at most `MAX_LEN` units long,
    // so every read up to its NUL is inside it.
    while len < MAX_LEN && unsafe { line.add(len).read() } != 0 {
        len += 1;
    }
    // SAFETY: the loop read these units, which nothing writes.
    unsafe { slice::from_raw_parts(line, len) }
}

/// Splits a command line into arguments by the rules of the Microsoft C
/// runtime since 2008, which std follows, and returns their count.
///
/// The program name ends at the first blank outside quotes, and its quotes
/// are dropped. In the arguments, blanks outside quotes separate; a quote
/// toggles quoting, and inside quotes a doubled quote is a literal one;
/// backslashes before a quote are halved, and an odd one out escapes it;
/// other backslashes are literal.
fn parse_command_line(line: &[u16], out: &mut Vec<u16>) -> usize {
    let mut units = line.iter().copied().peekable();
    let mut quoted = false;
    for unit in units.by_ref() {
        match unit {
            QUOTE => quoted = !quoted,
            SPACE | TAB if !quoted => break,
            _ => out.push(unit),
        }
    }
    out.push(0);
    let mut count = 1;
    while units
        .next_if(|&unit| unit == SPACE || unit == TAB)
        .is_some()
    {}
    quoted = false;
    while let Some(unit) = units.next() {
        match unit {
            SPACE | TAB if !quoted => {
                out.push(0);
                count += 1;
                while units.next_if(|&u| u == SPACE || u == TAB).is_some() {}
            }
            BACKSLASH => {
                let mut run = 1;
                while units.next_if_eq(&BACKSLASH).is_some() {
                    run += 1;
                }
                if units.peek() == Some(&QUOTE) {
                    out.extend(iter::repeat_n(BACKSLASH, run / 2));
                    if run % 2 == 1 {
                        units.next();
                        out.push(QUOTE);
                    }
                } else {
                    out.extend(iter::repeat_n(BACKSLASH, run));
                }
            }
            QUOTE if quoted => match units.peek() {
                Some(&QUOTE) => {
                    units.next();
                    out.push(QUOTE);
                }
                Some(_) => quoted = false,
                // A quote closing the line still makes an argument, even an
                // empty one; `quoted` stays set to say so.
                None => break,
            },
            QUOTE => quoted = true,
            _ => out.push(unit),
        }
    }
    if quoted || out.last() != Some(&0) {
        out.push(0);
        count += 1;
    }
    count
}

/// A snapshot of the environment: each `KEY=VALUE` entry, followed by a
/// NUL, and their count. Entries without a `=` after their first unit are
/// left out, as std leaves them out; names such as `=C:` start with one.
pub(crate) fn vars() -> (Vec<u16>, usize) {
    let strings = match EnvironmentStrings::get() {
        Ok(strings) => strings,
        Err(error) => no_environment(&error),
    };
    let mut out = Vec::with_capacity(strings.units().len());
    let mut count = 0;
    for (name, value) in strings.vars() {
        push_var(&mut out, name, value);
        count += 1;
    }
    (out, count)
}

/// Returns the value of the variable `key`, or `None` if it is not set or
/// `key` contains a NUL.
pub(crate) fn getenv(key: &OsStr) -> Option<OsString> {
    let mut buf = WideBuf::new();
    let key = to_wide(&mut buf, key, &[]).ok()?;
    fill_os_string(&mut |buf, size| {
        // SAFETY: `key` is NUL-terminated and `buf` valid for writes of
        // `size` units.
        unsafe { GetEnvironmentVariableW(key.as_ptr(), buf, size) }
    })
    .ok()
}

/// Sets `key` to `value`. Returns `false` if Windows refuses the pair or
/// either contains a NUL.
///
/// # Safety
///
/// None: Windows synchronizes access to the environment. The function is
/// unsafe to match the Unix one.
pub(crate) unsafe fn setenv(key: &OsStr, value: &OsStr) -> bool {
    let (mut key_buf, mut value_buf) = (WideBuf::new(), WideBuf::new());
    let key = to_wide(&mut key_buf, key, &[]);
    let value = to_wide(&mut value_buf, value, &[]);
    let (Ok(key), Ok(value)) = (key, value) else {
        return false;
    };
    // SAFETY: both strings are NUL-terminated.
    unsafe { SetEnvironmentVariableW(key.as_ptr(), value.as_ptr()) != 0 }
}

/// Removes `key`. Returns `false` if Windows refuses the name or it
/// contains a NUL.
///
/// # Safety
///
/// As for [`setenv`].
pub(crate) unsafe fn unsetenv(key: &OsStr) -> bool {
    let mut buf = WideBuf::new();
    let Ok(key) = to_wide(&mut buf, key, &[]) else {
        return false;
    };
    // SAFETY: `key` is NUL-terminated; a null value removes the variable.
    unsafe { SetEnvironmentVariableW(key.as_ptr(), ptr::null()) != 0 }
}

/// A function with the signature of `GetTempPath2W` and `GetTempPathW`.
type TempPathFn = unsafe extern "system" fn(u32, *mut u16) -> u32;

/// `GetTempPath2W`, which Windows 10 has only since build 20348, or else
/// `GetTempPathW`, as std picks.
fn temp_path_fn() -> TempPathFn {
    // SAFETY: the name is NUL-terminated; `kernel32` is never unloaded, and
    // the handle needs no release.
    let kernel32 = unsafe { GetModuleHandleW(w!("kernel32.dll")) };
    if !kernel32.is_null() {
        // SAFETY: `kernel32` is a loaded module and the name a C string.
        let found = unsafe {
            GetProcAddress(kernel32, c"GetTempPath2W".as_ptr().cast())
        };
        if let Some(found) = found {
            // SAFETY: `GetTempPath2W` has the signature of `TempPathFn`.
            return unsafe {
                mem::transmute::<unsafe extern "system" fn() -> isize, TempPathFn>(
                    found,
                )
            };
        }
    }
    GetTempPathW
}

/// The temporary directory, or an empty path where std panics because
/// Windows cannot provide one.
pub(crate) fn temp_dir() -> PathBuf {
    let get_temp_path = temp_path_fn();
    let path = fill_os_string(&mut |buf, size| {
        // SAFETY: `buf` is valid for writes of `size` units.
        unsafe { get_temp_path(size, buf) }
    });
    match path {
        Ok(path) => PathBuf::from(path),
        Err(error) => no_temp_dir(&error),
    }
}

/// Panics as std does when Windows cannot provide the environment block.
#[cold]
#[track_caller]
#[allow(clippy::panic, reason = "std panics here")]
fn no_environment(error: &io::Error) -> ! {
    panic!("failure getting env string from OS: {error}")
}

/// Panics as std does, which unwraps the result, when Windows cannot provide
/// the temporary directory.
#[cold]
#[track_caller]
#[allow(clippy::panic, reason = "std panics here")]
fn no_temp_dir(error: &io::Error) -> ! {
    panic!("called `Result::unwrap()` on an `Err` value: {error:?}")
}

/// `USERPROFILE` unless empty, then the profile directory of the process's
/// user.
pub(crate) fn home_dir() -> Option<PathBuf> {
    getenv(OsStr::new("USERPROFILE"))
        .filter(|profile| !profile.is_empty())
        .or_else(profile_dir)
        .map(PathBuf::from)
}

/// `GetUserProfileDirectoryW` for the current process's token.
fn profile_dir() -> Option<OsString> {
    /// `GetCurrentProcessToken()`, the pseudo handle -4, which is never
    /// closed.
    const CURRENT_PROCESS_TOKEN: HANDLE =
        ptr::without_provenance_mut(usize::MAX - 3);
    fill_os_string(&mut |buf, size| {
        let mut needed = size;
        // SAFETY: `buf` is valid for writes of `size` units, and `needed`
        // for one write.
        let ok = unsafe {
            GetUserProfileDirectoryW(
                CURRENT_PROCESS_TOKEN,
                buf,
                &raw mut needed,
            )
        };
        // Unlike the other functions, it reports sizes with the NUL.
        if ok != 0 {
            return needed.saturating_sub(1);
        }
        if last_error() == ERROR_INSUFFICIENT_BUFFER {
            needed
        } else {
            0
        }
    })
    .ok()
}

pub(crate) fn getcwd() -> io::Result<PathBuf> {
    fill_os_string(&mut |buf, size| {
        // SAFETY: `buf` is valid for writes of `size` units.
        unsafe { GetCurrentDirectoryW(size, buf) }
    })
    .map(PathBuf::from)
}

pub(crate) fn chdir(path: &Path) -> io::Result<()> {
    let mut buf = WideBuf::new();
    let path = to_wide(&mut buf, path.as_os_str(), &[])?;
    // SAFETY: `path` is NUL-terminated.
    cvt(unsafe { SetCurrentDirectoryW(path.as_ptr()) })
}

pub(crate) fn current_exe() -> io::Result<PathBuf> {
    exe_path().map(PathBuf::from)
}
