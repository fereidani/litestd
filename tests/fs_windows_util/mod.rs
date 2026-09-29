//! Helpers shared by the Windows `fs` tests: junctions and symbolic links
//! made with std or the Windows API, for litestd to find.

#![allow(dead_code, reason = "each test file uses a subset")]
#![allow(
    clippy::redundant_pub_crate,
    reason = "`pub` would trip `unreachable_pub`"
)]

use core::ptr;
use std::{
    io,
    os::windows::{fs::OpenOptionsExt as _, io::AsRawHandle as _},
};

use windows_sys::Win32::System::IO::DeviceIoControl;

pub(crate) const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
pub(crate) const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
pub(crate) const ERROR_PRIVILEGE_NOT_HELD: i32 = 1314;

/// Creates a junction at `link`, a new directory, to the absolute directory
/// `target`, through `FSCTL_SET_REPARSE_POINT`. std can read junctions but
/// has no stable way to make them.
pub(crate) fn junction(target: &str, link: &str) -> io::Result<()> {
    const FSCTL_SET_REPARSE_POINT: u32 = 0x0009_00A4;
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;

    std::fs::create_dir(link)?;
    let dir = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(link)?;
    let name: Vec<u16> = format!(r"\??\{target}").encode_utf16().collect();
    let too_long = || io::Error::from(io::ErrorKind::InvalidInput);
    let name_bytes = u16::try_from(name.len() * 2).map_err(|_| too_long())?;
    // The header, then the substitute name and an empty print name, each
    // NUL-terminated.
    let mut data = Vec::new();
    data.extend_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    data.extend_from_slice(&(8 + name_bytes + 4).to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    for field in [0, name_bytes, name_bytes + 2, 0] {
        data.extend_from_slice(&u16::to_le_bytes(field));
    }
    for unit in name.iter().chain(&[0, 0]) {
        data.extend_from_slice(&unit.to_le_bytes());
    }
    let len = u32::try_from(data.len()).map_err(|_| too_long())?;
    let mut returned = 0;
    // SAFETY: `data` holds a mount point reparse buffer of `len` bytes,
    // `returned` is valid for writes, and the handle is an open directory
    // for synchronous I/O.
    let ok = unsafe {
        DeviceIoControl(
            dir.as_raw_handle(),
            FSCTL_SET_REPARSE_POINT,
            data.as_ptr().cast(),
            len,
            ptr::null_mut(),
            0,
            &raw mut returned,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Creates a symbolic link with std. Returns `false` where the process may
/// not create symbolic links: that takes a privilege, or Developer Mode.
pub(crate) fn symlink(
    original: &str,
    link: &str,
    dir: bool,
) -> io::Result<bool> {
    let result = if dir {
        std::os::windows::fs::symlink_dir(original, link)
    } else {
        std::os::windows::fs::symlink_file(original, link)
    };
    match result {
        Ok(()) => Ok(true),
        Err(e) if e.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD) => {
            Ok(false)
        }
        Err(e) => Err(e),
    }
}
