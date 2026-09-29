//! Terminal detection for `io::IsTerminal`: a console, or the pipe of an
//! msys or cygwin pseudo-terminal.

use core::ffi::c_void;

use windows_sys::Win32::{
    Foundation::{HANDLE, MAX_PATH},
    Storage::FileSystem::{
        FILE_TYPE_PIPE, FileNameInfo, GetFileInformationByHandleEx, GetFileType,
    },
    System::Console::GetConsoleMode,
};

/// Whether `handle` is a console, or looks like an msys or cygwin
/// pseudo-terminal. A null handle, which a process without a console has
/// for its standard streams, is neither.
pub(crate) fn is_terminal(handle: HANDLE) -> bool {
    if handle.is_null() {
        return false;
    }
    let mut mode = 0;
    // SAFETY: `mode` is valid for writes; any handle value may be queried.
    if unsafe { GetConsoleMode(handle, &raw mut mode) } != 0 {
        return true;
    }
    is_msys_pty(handle)
}

/// `FILE_NAME_INFO` with room for a name of `MAX_PATH` units.
#[repr(C)]
struct NameInfo {
    /// The length of the name in bytes.
    len: u32,
    name: [u16; MAX_PATH as usize],
}

/// The size of `NameInfo`, 524 bytes.
#[allow(clippy::cast_possible_truncation, reason = "a small size")]
const NAME_INFO_SIZE: u32 = size_of::<NameInfo>() as u32;

/// std's heuristic for the pseudo-terminals of msys and cygwin, which are
/// named pipes: the last component of the pipe's name starts with `msys-`
/// or `cygwin-` and contains `-pty`.
fn is_msys_pty(handle: HANDLE) -> bool {
    // SAFETY: any handle value may be queried.
    if unsafe { GetFileType(handle) } != FILE_TYPE_PIPE {
        return false;
    }
    let mut info = NameInfo {
        len: 0,
        name: [0; MAX_PATH as usize],
    };
    // SAFETY: `info` is a writable `FILE_NAME_INFO` of the size passed.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileNameInfo,
            (&raw mut info).cast::<c_void>(),
            NAME_INFO_SIZE,
        )
    };
    if ok == 0 {
        return false;
    }
    // A length beyond the buffer means a truncated name, not a match.
    let Some(name) = info.name.get(..info.len as usize / 2) else {
        return false;
    };
    let name = name.rsplit(|&unit| unit == u16::from(b'\\')).next();
    name.is_some_and(|name| {
        (starts_with(name, b"msys-") || starts_with(name, b"cygwin-"))
            && name.windows(4).any(|w| eq_ascii(w, b"-pty"))
    })
}

/// Whether the UTF-16 `name` starts with the ASCII `prefix`.
fn starts_with(name: &[u16], prefix: &[u8]) -> bool {
    name.get(..prefix.len())
        .is_some_and(|head| eq_ascii(head, prefix))
}

/// Whether the UTF-16 `units` spell the ASCII `ascii`.
fn eq_ascii(units: &[u16], ascii: &[u8]) -> bool {
    units.len() == ascii.len()
        && units.iter().zip(ascii).all(|(&u, &a)| u == u16::from(a))
}
