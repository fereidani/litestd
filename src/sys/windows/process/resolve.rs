//! Finding the program to run as std does, and converting paths to the
//! forms that `CreateProcessW` and `cmd.exe` accept.

use core::ptr;

use alloc_crate::vec::Vec;
use windows_sys::{
    Win32::{
        Storage::FileSystem::{GetFileAttributesW, INVALID_FILE_ATTRIBUTES},
        System::{
            Environment::GetEnvironmentVariableW,
            LibraryLoader::GetModuleFileNameW,
            SystemInformation::{GetSystemDirectoryW, GetWindowsDirectoryW},
        },
    },
    core::w,
};

use super::{
    super::{
        os::wide::fill_os_string,
        path::{
            NT_PREFIX, NativePath, SEP, VERBATIM, VERBATIM_UNC, WideBuf,
            full_path, is_full_path, keeps_form, to_path_buf, to_u16s,
            user_form,
        },
    },
    args::{QUOTE, ensure_no_nuls, push_wide},
};
use crate::{
    ffi::OsStr,
    io,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};

const SEMICOLON: u16 = b';' as u16;

/// Returns the path that a size-reporting Windows function writes.
fn fill_path(f: &mut dyn FnMut(*mut u16, u32) -> u32) -> io::Result<PathBuf> {
    fill_os_string(f).map(PathBuf::from)
}

/// Returns the system directory, `C:\Windows\System32` by default.
fn system_dir() -> io::Result<PathBuf> {
    // SAFETY: `buf` is valid for writes of `size` units.
    fill_path(&mut |buf, size| unsafe { GetSystemDirectoryW(buf, size) })
}

/// Returns the Windows directory, `C:\Windows` by default.
fn windows_dir() -> io::Result<PathBuf> {
    // SAFETY: `buf` is valid for writes of `size` units.
    fill_path(&mut |buf, size| unsafe { GetWindowsDirectoryW(buf, size) })
}

/// Returns the directory of this process's program.
fn app_dir() -> io::Result<PathBuf> {
    // SAFETY: a null module is the program of this process, and `buf` is
    // valid for writes of `size` units.
    let mut path = fill_path(&mut |buf, size| unsafe {
        GetModuleFileNameW(ptr::null_mut(), buf, size)
    })?;
    path.pop();
    Ok(path)
}

/// Returns `path` (NUL-terminated) in the form std's `to_user_path` gives
/// it: without a verbatim prefix where one is not needed, since `cmd.exe`
/// does not accept them, and absolute.
pub(super) fn to_user_path(path: &OsStr) -> io::Result<Vec<u16>> {
    let mut wide = to_u16s(&mut WideBuf::new(), path)?.to_vec();
    if user_form(&mut wide)? {
        return Ok(wide);
    }
    long_path(path, wide)
}

/// Makes `wide`, the NUL-terminated form of `path`, absolute as std's
/// `get_long_path` does without preferring verbatim paths: short absolute
/// and verbatim paths stay, others go through `GetFullPathNameW`, and get a
/// verbatim prefix only if too long for the legacy APIs.
fn long_path(path: &OsStr, wide: Vec<u16>) -> io::Result<Vec<u16>> {
    /// The legacy limit of directory APIs, from which std adds the prefix.
    const LEGACY_DIR_MAX: usize = 248;
    if keeps_form(&wide) {
        return Ok(wide);
    }
    let mut absolute = full_path(&mut WideBuf::new(), &wide)?.to_vec();
    if absolute.len() + 1 < LEGACY_DIR_MAX {
        absolute.push(0);
        return Ok(absolute);
    }
    // Rare: the same conversion with the verbatim prefix.
    Ok(NativePath::new().convert(path)?.to_vec())
}

/// Returns the user path of `path` if something exists there, not following
/// symbolic links.
fn program_exists(path: &Path) -> Option<Vec<u16>> {
    let wide = to_user_path(path.as_os_str()).ok()?;
    // SAFETY: `wide` is NUL-terminated.
    let attributes = unsafe { GetFileAttributesW(wide.as_ptr()) };
    (attributes != INVALID_FILE_ATTRIBUTES).then_some(wide)
}

/// Returns the program that `exe` names, NUL-terminated, as std resolves
/// it. A path is used as it is, or with `.exe` appended if that exists.
/// A file name is searched in `child_paths` (the child's `PATH` if the
/// command changed it), the directory of this program, the system and
/// Windows directories, and this process's `PATH`, with `.exe` appended
/// if it has no extension.
pub(super) fn resolve_exe(
    exe: &OsStr,
    child_paths: Option<&OsStr>,
) -> io::Result<Vec<u16>> {
    let bytes = exe.as_encoded_bytes();
    let verbatim = bytes.starts_with(br"\\?\");
    let trailing_sep = match bytes.last() {
        Some(b'\\') => true,
        Some(b'/') => !verbatim,
        _ => false,
    };
    if bytes.is_empty() || trailing_sep {
        return Err(io::const_error!(
            io::ErrorKind::InvalidInput,
            "program path has no file name",
        ));
    }
    if bytes.iter().any(|&b| matches!(b, b'\\' | b'/')) {
        let has_exe_suffix = bytes
            .last_chunk::<4>()
            .is_some_and(|ext| ext.eq_ignore_ascii_case(b".exe"));
        if has_exe_suffix {
            // `CreateProcessW` reports whether it exists.
            return to_user_path(exe);
        }
        let mut path = PathBuf::from(exe);
        path.as_mut_os_string().push(".exe");
        if let Some(found) = program_exists(&path) {
            return Ok(found);
        }
        // Removes the `.exe` again, as std does.
        path.set_extension("");
        return to_user_path(path.as_os_str());
    }
    ensure_no_nuls(exe)?;
    let search = Search {
        exe,
        // `CreateProcessW` appends `.exe` to a name without an extension
        // only when it searches, and so does std.
        add_exe: !bytes.contains(&b'.'),
    };
    search.run(child_paths).ok_or(io::const_error!(
        io::ErrorKind::NotFound,
        "program not found",
    ))
}

/// A search for a program by its file name.
struct Search<'a> {
    exe: &'a OsStr,
    add_exe: bool,
}

impl Search<'_> {
    /// Searches the directories in std's order.
    fn run(&self, child_paths: Option<&OsStr>) -> Option<Vec<u16>> {
        if let Some(paths) = child_paths {
            let wide: Vec<u16> = paths.encode_wide().collect();
            if let Some(found) = self.in_list(&wide) {
                return Some(found);
            }
        }
        let dirs: [fn() -> io::Result<PathBuf>; 3] =
            [app_dir, system_dir, windows_dir];
        for dir in dirs {
            let found = dir().ok().and_then(|dir| self.in_dir(dir));
            if found.is_some() {
                return found;
            }
        }
        let mut buf = WideBuf::new();
        // Unset, `PATH` fails with `ERROR_ENVVAR_NOT_FOUND`.
        let path_var = buf.fill_wide(&mut |out, size| {
            // SAFETY: the name is NUL-terminated, and `out` is valid for
            // writes of `size` units.
            unsafe { GetEnvironmentVariableW(w!("PATH"), out, size) }
        });
        self.in_list(path_var.ok()?)
    }

    /// Searches each nonempty directory of the `PATH`-style list `paths`,
    /// split at semicolons outside quotes, and without the quotes, as std's
    /// `env::split_paths` does.
    fn in_list(&self, paths: &[u16]) -> Option<Vec<u16>> {
        let mut dir = Vec::new();
        let mut in_quote = false;
        // `None` marks the end, which ends the last directory.
        for unit in paths.iter().copied().map(Some).chain([None]) {
            match unit {
                Some(QUOTE) => in_quote = !in_quote,
                Some(unit) if unit != SEMICOLON || in_quote => dir.push(unit),
                _ => {
                    if !dir.is_empty() {
                        let found = self.in_dir(to_path_buf(&dir));
                        if found.is_some() {
                            return found;
                        }
                    }
                    dir.clear();
                }
            }
        }
        None
    }

    /// Returns the program in `dir`, if it exists there.
    fn in_dir(&self, mut dir: PathBuf) -> Option<Vec<u16>> {
        dir.push(self.exe);
        if self.add_exe {
            dir.set_extension("exe");
        }
        program_exists(&dir)
    }
}

/// Whether `program` (NUL-terminated) is a batch file, which runs through
/// `cmd.exe`. As in std, the extension is checked on the full path, since
/// Windows drops trailing dots and spaces: `a.bat. .` runs `a.bat`.
pub(super) fn is_batch_file(program: &[u16]) -> io::Result<bool> {
    /// Whether `path` ends in `.bat` or `.cmd`, in any case.
    fn has_batch_extension(path: &[u16]) -> bool {
        let Some(&[dot, a, b, c]) = path.last_chunk() else {
            return false;
        };
        let lower = [a, b, c].map(|unit| {
            u8::try_from(unit).map_or(0, |b| b.to_ascii_lowercase())
        });
        dot == u16::from(b'.') && matches!(&lower, b"bat" | b"cmd")
    }
    if program.starts_with(&VERBATIM) || program.starts_with(&NT_PREFIX) {
        let path = program.split_last().map_or(program, |(_, path)| path);
        return Ok(has_batch_extension(path));
    }
    full_path(&mut WideBuf::new(), program).map(has_batch_extension)
}

/// Returns the NUL-terminated path of `cmd.exe` in the system directory.
pub(super) fn command_prompt() -> io::Result<Vec<u16>> {
    let dir = system_dir()?;
    let mut path = Vec::with_capacity(dir.as_os_str().len() + 9);
    push_wide(&mut path, dir.as_os_str());
    path.extend(br"\cmd.exe".iter().map(|&b| u16::from(b)));
    path.push(0);
    Ok(path)
}

/// Returns the NUL-terminated working directory for `CreateProcessW` and
/// where it starts: without a verbatim prefix if the rest means the same,
/// since the working directory cannot be verbatim.
pub(super) fn working_dir(dir: &OsStr) -> io::Result<(Vec<u16>, usize)> {
    ensure_no_nuls(dir)?;
    let mut wide = Vec::with_capacity(dir.len() + 1);
    push_wide(&mut wide, dir);
    wide.push(0);
    // `\\?\UNC`, without the separator that follows it.
    let start = if wide.starts_with(&VERBATIM_UNC[..7]) {
        // `\\?\UNC\server` becomes `\\server`.
        if let Some(unit) = wide.get_mut(6) {
            *unit = SEP;
        }
        6
    } else if wide.starts_with(&VERBATIM) {
        4
    } else {
        return Ok((wide, 0));
    };
    // A failure counts as a path that changes, as in std.
    if is_full_path(wide.get(start..).unwrap_or_default()).unwrap_or(false) {
        return Ok((wide, start));
    }
    if let (6, Some(unit)) = (start, wide.get_mut(6)) {
        *unit = b'C'.into();
    }
    Ok((wide, 0))
}
