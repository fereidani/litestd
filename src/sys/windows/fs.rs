//! File system operations on top of Windows file handles, with std's
//! outcomes (flags, access rights, fallbacks and error codes) and fewer
//! system calls where the outcome allows.

use core::{mem::offset_of, ptr};

use alloc_crate::vec::Vec;
use windows_sys::{
    Wdk::Storage::FileSystem::{
        FILE_RENAME_POSIX_SEMANTICS, FILE_RENAME_REPLACE_IF_EXISTS,
        SYMLINK_FLAG_RELATIVE,
    },
    Win32::{
        Foundation::{
            ERROR_ACCESS_DENIED, ERROR_CANT_ACCESS_FILE, ERROR_DIR_NOT_EMPTY,
            ERROR_FILE_NOT_FOUND, ERROR_INVALID_PARAMETER,
            ERROR_INVALID_REPARSE_DATA, ERROR_SHARING_VIOLATION, FILETIME,
        },
        Storage::FileSystem::{
            CopyFileExW, CreateDirectoryW, CreateHardLinkW,
            CreateSymbolicLinkW, DELETE, DeleteFileW, FILE_ATTRIBUTE_DIRECTORY,
            FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_REPARSE_POINT,
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_RENAME_INFO, FILE_WRITE_ATTRIBUTES, FileRenameInfoEx,
            GetFileAttributesExW, GetFileExInfoStandard,
            MAXIMUM_REPARSE_DATA_BUFFER_SIZE, MOVEFILE_REPLACE_EXISTING,
            MoveFileExW, PROGRESS_CONTINUE, RemoveDirectoryW,
            SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE,
            SYMBOLIC_LINK_FLAG_DIRECTORY, SetFileAttributesW,
            SetFileInformationByHandle, WIN32_FILE_ATTRIBUTE_DATA,
            WIN32_FIND_DATAW,
        },
        System::IO::DeviceIoControl,
    },
};

use super::{
    dir::find_first,
    file::{final_path, posix_delete},
    os::{cvt, last_error, os_code, win_error},
    path::{
        COLON, DOT, NT_PREFIX, NativePath, SEP, VERBATIM, VERBATIM_UNC,
        WideBuf, to_path_buf, to_u16s, user_form,
    },
    time::{INTERVALS_PER_SEC, SECS_TO_UNIX_EPOCH},
};
pub(crate) use super::{
    dir::{DirEntry, ReadDir, readdir},
    file::{File, OpenOptions},
    remove_dir_all::remove_dir_all,
};
use crate::{
    io,
    os::windows::io::AsRawHandle,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Reinterprets the bits of a `u64` as an `i64`, as the Windows API does.
pub(crate) const fn signed(n: u64) -> i64 {
    #[allow(clippy::cast_possible_wrap)]
    let n = n as i64;
    n
}

/// Reinterprets the bits of an `i64` as a `u64`.
pub(crate) const fn unsigned(n: i64) -> u64 {
    #[allow(clippy::cast_sign_loss)]
    let n = n as u64;
    n
}

/// Metadata information about a file.
#[derive(Clone, Copy)]
pub(crate) struct FileAttr {
    pub(super) attributes: u32,
    /// The reparse tag if the file is a reparse point, and 0 otherwise.
    pub(super) reparse_tag: u32,
    pub(super) creation_time: u64,
    pub(super) last_access_time: u64,
    pub(super) last_write_time: u64,
    pub(super) file_size: u64,
}

/// The 64-bit value of a `FILETIME`.
const fn filetime_u64(ft: FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

impl FileAttr {
    /// Metadata from a directory listing, as std reads it.
    pub(crate) const fn from_find_data(data: &WIN32_FIND_DATAW) -> Self {
        let attributes = data.dwFileAttributes;
        Self {
            attributes,
            // The field is reserved unless the file is a reparse point.
            reparse_tag: if attributes & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
                0
            } else {
                data.dwReserved0
            },
            creation_time: filetime_u64(data.ftCreationTime),
            last_access_time: filetime_u64(data.ftLastAccessTime),
            last_write_time: filetime_u64(data.ftLastWriteTime),
            file_size: ((data.nFileSizeHigh as u64) << 32)
                | data.nFileSizeLow as u64,
        }
    }

    /// Metadata from `GetFileAttributesExW` of a non-reparse-point file.
    const fn from_attribute_data(data: &WIN32_FILE_ATTRIBUTE_DATA) -> Self {
        Self {
            attributes: data.dwFileAttributes,
            reparse_tag: 0,
            creation_time: filetime_u64(data.ftCreationTime),
            last_access_time: filetime_u64(data.ftLastAccessTime),
            last_write_time: filetime_u64(data.ftLastWriteTime),
            file_size: ((data.nFileSizeHigh as u64) << 32)
                | data.nFileSizeLow as u64,
        }
    }

    pub(crate) const fn size(&self) -> u64 {
        self.file_size
    }

    pub(crate) const fn perm(&self) -> FilePermissions {
        FilePermissions {
            attrs: self.attributes,
        }
    }

    pub(crate) const fn file_type(&self) -> FileType {
        FileType::new(self.attributes, self.reparse_tag)
    }

    #[allow(clippy::unnecessary_wraps)]
    pub(crate) fn modified(&self) -> io::Result<SystemTime> {
        Ok(system_time(self.last_write_time))
    }

    #[allow(clippy::unnecessary_wraps)]
    pub(crate) fn accessed(&self) -> io::Result<SystemTime> {
        Ok(system_time(self.last_access_time))
    }

    #[allow(clippy::unnecessary_wraps)]
    pub(crate) fn created(&self) -> io::Result<SystemTime> {
        Ok(system_time(self.creation_time))
    }

    pub(crate) const fn file_attributes(&self) -> u32 {
        self.attributes
    }

    pub(crate) const fn creation_time(&self) -> u64 {
        self.creation_time
    }

    pub(crate) const fn last_access_time(&self) -> u64 {
        self.last_access_time
    }

    pub(crate) const fn last_write_time(&self) -> u64 {
        self.last_write_time
    }
}

/// Converts a `FILETIME`, read as signed as std does, to a `SystemTime`.
fn system_time(filetime: u64) -> SystemTime {
    let intervals = signed(filetime);
    let secs = intervals.div_euclid(INTERVALS_PER_SEC) - SECS_TO_UNIX_EPOCH;
    // Below one billion, which `from_unix` accepts.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let nanos = (intervals.rem_euclid(INTERVALS_PER_SEC) * 100) as u32;
    SystemTime::from_unix(secs, nanos).unwrap_or(UNIX_EPOCH)
}

/// Converts a `SystemTime` to a `FILETIME` value, rounding toward the Unix
/// epoch as std does. Times outside std's range, 0 to `i64::MAX`
/// intervals, fail with `InvalidInput`.
pub(crate) fn filetime(t: SystemTime) -> io::Result<u64> {
    let (secs, nanos) = t.to_unix();
    let floor = secs
        .checked_add(SECS_TO_UNIX_EPOCH)
        .and_then(|s| s.checked_mul(INTERVALS_PER_SEC))
        .and_then(|i| i.checked_add(i64::from(nanos / 100)));
    // Before the epoch, toward it is up.
    let intervals = if secs < 0 && nanos % 100 != 0 {
        floor.and_then(|i| i.checked_add(1))
    } else {
        floor
    };
    match intervals {
        Some(n) if n >= 0 => Ok(unsigned(n)),
        _ => Err(io::const_error!(
            io::ErrorKind::InvalidInput,
            "timestamp cannot be represented as a FILETIME",
        )),
    }
}

/// A file's attributes, of which only the read-only one is a permission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FilePermissions {
    attrs: u32,
}

impl FilePermissions {
    pub(crate) const fn readonly(self) -> bool {
        self.attrs & FILE_ATTRIBUTE_READONLY != 0
    }

    pub(crate) const fn set_readonly(&mut self, readonly: bool) {
        if readonly {
            self.attrs |= FILE_ATTRIBUTE_READONLY;
        } else {
            self.attrs &= !FILE_ATTRIBUTE_READONLY;
        }
    }

    pub(crate) const fn attributes(self) -> u32 {
        self.attrs
    }
}

/// The type of a file: a directory, a symbolic link, which is any name
/// surrogate reparse point, junctions included, or a file.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct FileType {
    is_directory: bool,
    is_symlink: bool,
}

impl FileType {
    pub(crate) const fn new(attributes: u32, reparse_tag: u32) -> Self {
        /// Marks the reparse tags of links to another name.
        const NAME_SURROGATE: u32 = 0x2000_0000;
        Self {
            is_directory: attributes & FILE_ATTRIBUTE_DIRECTORY != 0,
            is_symlink: attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
                && reparse_tag & NAME_SURROGATE != 0,
        }
    }

    pub(crate) const fn is_dir(self) -> bool {
        !self.is_symlink && self.is_directory
    }

    pub(crate) const fn is_file(self) -> bool {
        !self.is_symlink && !self.is_directory
    }

    pub(crate) const fn is_symlink(self) -> bool {
        self.is_symlink
    }

    pub(crate) const fn is_symlink_dir(self) -> bool {
        self.is_symlink && self.is_directory
    }

    pub(crate) const fn is_symlink_file(self) -> bool {
        self.is_symlink && !self.is_directory
    }
}

/// The times to set on a file; `None` leaves a time unchanged.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FileTimes {
    pub(super) accessed: Option<SystemTime>,
    pub(super) modified: Option<SystemTime>,
    pub(super) created: Option<SystemTime>,
}

impl FileTimes {
    pub(crate) const fn set_accessed(&mut self, t: SystemTime) {
        self.accessed = Some(t);
    }

    pub(crate) const fn set_modified(&mut self, t: SystemTime) {
        self.modified = Some(t);
    }

    pub(crate) const fn set_created(&mut self, t: SystemTime) {
        self.created = Some(t);
    }
}

/// A builder used to create directories.
#[derive(Debug)]
pub(crate) struct DirBuilder;

impl DirBuilder {
    pub(crate) const fn new() -> Self {
        Self
    }

    #[allow(clippy::unused_self)]
    pub(crate) fn mkdir(&self, path: &Path) -> io::Result<()> {
        let mut native = NativePath::new();
        let path = native.convert(path.as_os_str())?;
        // SAFETY: `path` is NUL-terminated; no security attributes.
        cvt(unsafe { CreateDirectoryW(path.as_ptr(), ptr::null()) })
    }
}

pub(crate) fn unlink(path: &Path) -> io::Result<()> {
    let mut native = NativePath::new();
    let path = native.convert(path.as_os_str())?;
    // SAFETY: `path` is NUL-terminated.
    if unsafe { DeleteFileW(path.as_ptr()) } != 0 {
        return Ok(());
    }
    let error = last_error();
    // `DeleteFileW` refuses read-only files, which a POSIX deletion
    // removes anyway, as in std.
    if error == ERROR_ACCESS_DENIED {
        let flags = FILE_FLAG_OPEN_REPARSE_POINT;
        if let Ok(file) = File::open_existing(path, DELETE, flags) {
            if posix_delete(file.handle().as_raw_handle()).is_ok() {
                return Ok(());
            }
        }
    }
    Err(win_error(error))
}

pub(crate) fn rename(old: &Path, new: &Path) -> io::Result<()> {
    let mut old_native = NativePath::new();
    let old = old_native.convert(old.as_os_str())?;
    let mut new_native = NativePath::new();
    let new = new_native.convert(new.as_os_str())?;
    // SAFETY: both paths are NUL-terminated.
    let ok = unsafe {
        MoveFileExW(old.as_ptr(), new.as_ptr(), MOVEFILE_REPLACE_EXISTING)
    };
    if ok != 0 {
        return Ok(());
    }
    let error = last_error();
    if error == ERROR_ACCESS_DENIED {
        rename_posix(old, new, error)
    } else {
        Err(win_error(error))
    }
}

/// Retries a rename that `MoveFileExW` refused, as std does, with POSIX
/// semantics, which replace a read-only file too. Fails with `error`, the
/// error of `MoveFileExW`, unless the target is a non-empty directory.
#[cold]
fn rename_posix(old: &[u16], new: &[u16], error: u32) -> io::Result<()> {
    let open_flags = FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS;
    let Ok(file) = File::open_existing(old, DELETE, open_flags) else {
        return Err(win_error(error));
    };
    // `FILE_RENAME_INFO` ends in the new name, which `FileNameLength`
    // measures in bytes without the NUL that follows it.
    let name_offset = offset_of!(FILE_RENAME_INFO, FileName);
    let size = name_offset + new.len() * 2;
    let Ok(size_u32) = u32::try_from(size) else {
        return Err(win_error(error));
    };
    let name_len = u32::try_from(new.len().saturating_sub(1) * 2).unwrap_or(0);
    let mut bytes = alloc_crate::vec![0_u8; size];
    let flags = FILE_RENAME_REPLACE_IF_EXISTS | FILE_RENAME_POSIX_SEMANTICS;
    let fields = [
        (offset_of!(FILE_RENAME_INFO, Anonymous), flags),
        (offset_of!(FILE_RENAME_INFO, FileNameLength), name_len),
    ];
    for (offset, value) in fields {
        if let Some(field) = bytes.get_mut(offset..offset + 4) {
            field.copy_from_slice(&value.to_ne_bytes());
        }
    }
    let name = bytes.get_mut(name_offset..).unwrap_or_default();
    for (slot, unit) in name.chunks_exact_mut(2).zip(new) {
        slot.copy_from_slice(&unit.to_ne_bytes());
    }
    // The structure holds a handle, so it goes to the kernel as aligned `u64`s.
    let buf: Vec<u64> = bytes
        .chunks(8)
        .map(|chunk| {
            let mut word = [0; 8];
            for (to, from) in word.iter_mut().zip(chunk) {
                *to = *from;
            }
            u64::from_ne_bytes(word)
        })
        .collect();
    // SAFETY: the first `size_u32` bytes of `buf` hold an initialized,
    // aligned `FILE_RENAME_INFO` and its name, as `FileRenameInfoEx` takes;
    // the root directory is null, and the handle is open.
    let ok = unsafe {
        SetFileInformationByHandle(
            file.handle().as_raw_handle(),
            FileRenameInfoEx,
            buf.as_ptr().cast(),
            size_u32,
        )
    };
    if ok != 0 {
        return Ok(());
    }
    if last_error() == ERROR_DIR_NOT_EMPTY {
        return Err(win_error(ERROR_DIR_NOT_EMPTY));
    }
    Err(win_error(error))
}

pub(crate) fn rmdir(path: &Path) -> io::Result<()> {
    let mut native = NativePath::new();
    let path = native.convert(path.as_os_str())?;
    // SAFETY: `path` is NUL-terminated.
    cvt(unsafe { RemoveDirectoryW(path.as_ptr()) })
}

pub(crate) fn readlink(path: &Path) -> io::Result<PathBuf> {
    /// From `winioctl.h`, to avoid windows-sys's large `Ioctl` module.
    const FSCTL_GET_REPARSE_POINT: u32 = 0x0009_00A8;
    const BUF_BYTES: usize = MAXIMUM_REPARSE_DATA_BUFFER_SIZE as usize;

    let mut native = NativePath::new();
    let path = native.convert(path.as_os_str())?;
    // No access rights: some junctions, such as `C:\Documents and
    // Settings`, deny listing their directory.
    let flags = FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS;
    let file = File::open_existing(path, 0, flags)?;
    let mut buf = alloc_crate::vec![0_u8; BUF_BYTES];
    let mut len = 0;
    // SAFETY: `buf` (`BUF_BYTES` bytes) and `len` are valid for writes. The
    // request is buffered, so `buf` needs no alignment, and the handle is
    // synchronous, so the call completes before it returns.
    cvt(unsafe {
        DeviceIoControl(
            file.handle().as_raw_handle(),
            FSCTL_GET_REPARSE_POINT,
            ptr::null(),
            0,
            buf.as_mut_ptr().cast(),
            MAXIMUM_REPARSE_DATA_BUFFER_SIZE,
            &raw mut len,
            ptr::null_mut(),
        )
    })?;
    let data = buf.get(..len as usize).unwrap_or(&buf);
    let (mut target, relative) = link_target(data)?;
    // Absolute targets are in the NT namespace; `\??\` becomes `\\?\`, and
    // then the plain path if that names the same file.
    if !relative && target.starts_with(&NT_PREFIX) {
        if let Some(unit) = target.get_mut(1) {
            *unit = SEP;
        }
        target.push(0);
        user_form(&mut target)?;
        target.pop();
    }
    Ok(to_path_buf(&target))
}

/// Reads the substitute name of a symbolic link or a junction from its
/// reparse data, and whether it is relative.
fn link_target(data: &[u8]) -> io::Result<(Vec<u16>, bool)> {
    /// From `winnt.h`, to avoid windows-sys's large `SystemServices` module.
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;

    let u16_at = |at: usize| -> Option<usize> {
        let bytes = data.get(at..)?.first_chunk().copied();
        Some(usize::from(u16::from_ne_bytes(bytes?)))
    };
    let u32_at = |at: usize| -> Option<u32> {
        let bytes = data.get(at..)?.first_chunk().copied();
        Some(u32::from_ne_bytes(bytes?))
    };
    // The header: the tag, the data length and a reserved field, then the
    // substitute name's offset and length and the print name's, then, for
    // symbolic links only, the flags, then the names.
    let parsed = u32_at(0).and_then(|tag| {
        let (names, relative) = match tag {
            IO_REPARSE_TAG_SYMLINK => {
                (20, u32_at(16)? & SYMLINK_FLAG_RELATIVE != 0)
            }
            IO_REPARSE_TAG_MOUNT_POINT => (16, false),
            _ => return Some(Err(())),
        };
        let start = names + u16_at(8)?;
        let name = data.get(start..start + u16_at(10)?)?;
        let target = name
            .chunks_exact(2)
            .filter_map(|unit| <[u8; 2]>::try_from(unit).ok())
            .map(u16::from_ne_bytes)
            .collect();
        Some(Ok((target, relative)))
    });
    match parsed {
        Some(Ok(target)) => Ok(target),
        Some(Err(())) => Err(io::const_error!(
            io::ErrorKind::Uncategorized,
            "Unsupported reparse point type",
        )),
        None => Err(win_error(ERROR_INVALID_REPARSE_DATA)),
    }
}

/// Creates a symbolic link to a file, or a directory if `dir`.
pub(crate) fn symlink(
    original: &Path,
    link: &Path,
    dir: bool,
) -> io::Result<()> {
    // The target is stored as given, not made absolute or verbatim.
    let mut original_buf = WideBuf::new();
    let original = to_u16s(&mut original_buf, original.as_os_str())?;
    let mut link_native = NativePath::new();
    let link = link_native.convert(link.as_os_str())?;
    let flags = if dir { SYMBOLIC_LINK_FLAG_DIRECTORY } else { 0 };
    // Without the privilege to create symbolic links, Windows 10 and later
    // allow them in Developer Mode, but only with this flag.
    let unprivileged = flags | SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE;
    // SAFETY: both paths are NUL-terminated.
    if unsafe {
        CreateSymbolicLinkW(link.as_ptr(), original.as_ptr(), unprivileged)
    } {
        return Ok(());
    }
    let error = last_error();
    if error != ERROR_INVALID_PARAMETER {
        return Err(win_error(error));
    }
    // Windows before the Creators Update rejects the flag.
    // SAFETY: both paths are NUL-terminated.
    if unsafe { CreateSymbolicLinkW(link.as_ptr(), original.as_ptr(), flags) } {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(crate) fn link(original: &Path, link: &Path) -> io::Result<()> {
    let mut original_native = NativePath::new();
    let original = original_native.convert(original.as_os_str())?;
    let mut link_native = NativePath::new();
    let link = link_native.convert(link.as_os_str())?;
    // SAFETY: both paths are NUL-terminated; no security attributes.
    cvt(unsafe {
        CreateHardLinkW(link.as_ptr(), original.as_ptr(), ptr::null())
    })
}

/// Returns whether `native`, a converted path, names a file in a file
/// system through a drive or a share, rather than a device.
fn is_file_system_path(native: &[u16]) -> bool {
    let is_drive = |path: &[u16]| match *path {
        [letter, COLON, ..] => {
            u8::try_from(letter).is_ok_and(|c| c.is_ascii_alphabetic())
        }
        _ => false,
    };
    if let Some(rest) = native.strip_prefix(&VERBATIM) {
        return is_drive(rest) || native.starts_with(&VERBATIM_UNC);
    }
    !matches!(*native, [SEP, SEP, DOT, SEP, ..])
        && !native.starts_with(&NT_PREFIX)
}

/// Reads the metadata of the file at `native` without opening it, where that
/// gives std's answer: not a reparse point, or missing from an existing
/// directory. `None` leaves the answer to std's way of opening the file.
fn quick_stat(native: &[u16]) -> Option<io::Result<FileAttr>> {
    if !is_file_system_path(native) {
        return None;
    }
    let mut data = WIN32_FILE_ATTRIBUTE_DATA::default();
    // SAFETY: `native` is NUL-terminated, and `data` is valid for writes of
    // the structure that `GetFileExInfoStandard` fills in.
    let ok = unsafe {
        GetFileAttributesExW(
            native.as_ptr(),
            GetFileExInfoStandard,
            (&raw mut data).cast(),
        )
    };
    if ok == 0 {
        // A missing last component fails the same way for `CreateFileW`;
        // std's way decides other failures, which it may report differently.
        let error = last_error();
        return (error == ERROR_FILE_NOT_FOUND).then(|| Err(win_error(error)));
    }
    // A reparse point needs its tag, or following.
    (data.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0)
        .then(|| Ok(FileAttr::from_attribute_data(&data)))
}

pub(crate) fn stat(path: &Path) -> io::Result<FileAttr> {
    let mut native = NativePath::new();
    let path = native.convert(path.as_os_str())?;
    if let Some(result) = quick_stat(path) {
        return result;
    }
    match open_stat(path, false) {
        // `CreateFileW` could not follow the reparse point, as for Unix
        // sockets and app execution aliases; std then reports the file
        // itself if it is not a link.
        Err(e) if e.raw_os_error() == Some(os_code(ERROR_CANT_ACCESS_FILE)) => {
            match open_stat(path, true) {
                Ok(attr) if !attr.file_type().is_symlink() => Ok(attr),
                _ => Err(e),
            }
        }
        result => result,
    }
}

pub(crate) fn lstat(path: &Path) -> io::Result<FileAttr> {
    let mut native = NativePath::new();
    let path = native.convert(path.as_os_str())?;
    quick_stat(path).unwrap_or_else(|| open_stat(path, true))
}

/// Reads the metadata of the file at `native` through a handle, or of the
/// link itself if `no_follow`, as std does.
fn open_stat(native: &[u16], no_follow: bool) -> io::Result<FileAttr> {
    let mut flags = FILE_FLAG_BACKUP_SEMANTICS;
    if no_follow {
        flags |= FILE_FLAG_OPEN_REPARSE_POINT;
    }
    match File::open_existing(native, 0, flags) {
        Ok(file) => file.file_attr(),
        // Some system files, such as `C:\hiberfil.sys`, deny every open; a
        // wildcard-free search reads their metadata from the directory.
        Err(e)
            if matches!(
                e.raw_os_error(),
                Some(code) if code == os_code(ERROR_SHARING_VIOLATION)
                    || code == os_code(ERROR_ACCESS_DENIED)
            ) =>
        {
            let Ok((_, data)) = find_first(native, 0) else {
                return Err(e);
            };
            let attr = FileAttr::from_find_data(&data);
            if !no_follow && attr.file_type().is_symlink() {
                return Err(e);
            }
            Ok(attr)
        }
        Err(e) => Err(e),
    }
}

pub(crate) fn canonicalize(path: &Path) -> io::Result<PathBuf> {
    let mut native = NativePath::new();
    let path = native.convert(path.as_os_str())?;
    // Directories open only with backup semantics.
    let file = File::open_existing(path, 0, FILE_FLAG_BACKUP_SEMANTICS)?;
    final_path(file.handle().as_raw_handle())
}

pub(crate) fn copy(from: &Path, to: &Path) -> io::Result<u64> {
    /// Records the bytes copied of the main stream, stream 1.
    #[allow(clippy::missing_const_for_fn, reason = "called by the OS")]
    unsafe extern "system" fn progress(
        _total_size: i64,
        _total_transferred: i64,
        _stream_size: i64,
        stream_transferred: i64,
        stream_number: u32,
        _reason: u32,
        _source: *mut core::ffi::c_void,
        _destination: *mut core::ffi::c_void,
        data: *const core::ffi::c_void,
    ) -> u32 {
        if stream_number == 1 {
            // SAFETY: `data` is the pointer to the `i64` that `copy` passes
            // to `CopyFileExW`, which calls this during the copy only.
            unsafe { data.cast_mut().cast::<i64>().write(stream_transferred) };
        }
        PROGRESS_CONTINUE
    }

    let mut from_native = NativePath::new();
    let from = from_native.convert(from.as_os_str())?;
    let mut to_native = NativePath::new();
    let to = to_native.convert(to.as_os_str())?;
    let mut size: i64 = 0;
    // SAFETY: both paths are NUL-terminated, and `progress` matches the
    // callback type and writes only to `size`, which outlives the call.
    cvt(unsafe {
        CopyFileExW(
            from.as_ptr(),
            to.as_ptr(),
            Some(progress),
            (&raw mut size).cast_const().cast(),
            ptr::null_mut(),
            0,
        )
    })?;
    Ok(unsigned(size))
}

pub(crate) fn set_perm(path: &Path, perm: FilePermissions) -> io::Result<()> {
    let mut native = NativePath::new();
    let path = native.convert(path.as_os_str())?;
    // SAFETY: `path` is NUL-terminated.
    cvt(unsafe { SetFileAttributesW(path.as_ptr(), perm.attrs) })
}

pub(crate) fn set_times(path: &Path, times: FileTimes) -> io::Result<()> {
    set_times_impl(path, times, 0)
}

pub(crate) fn set_times_nofollow(
    path: &Path,
    times: FileTimes,
) -> io::Result<()> {
    set_times_impl(path, times, FILE_FLAG_OPEN_REPARSE_POINT)
}

fn set_times_impl(path: &Path, times: FileTimes, flags: u32) -> io::Result<()> {
    let mut native = NativePath::new();
    let path = native.convert(path.as_os_str())?;
    let flags = FILE_FLAG_BACKUP_SEMANTICS | flags;
    File::open_existing(path, FILE_WRITE_ATTRIBUTES, flags)?.set_times(times)
}

pub(crate) fn exists(path: &Path) -> io::Result<bool> {
    let mut native = NativePath::new();
    let path = native.convert(path.as_os_str())?;
    match quick_stat(path) {
        Some(Ok(_)) => return Ok(true),
        Some(Err(_)) => return Ok(false),
        None => {}
    }
    match File::open_existing(path, 0, FILE_FLAG_BACKUP_SEMANTICS) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        // A file locked by another process, usually for a short time.
        // Special files whose reparse point `CreateFileW` cannot follow,
        // such as Unix sockets and app execution aliases.
        Err(e)
            if matches!(
                e.raw_os_error(),
                Some(code) if code == os_code(ERROR_SHARING_VIOLATION)
                    || code == os_code(ERROR_CANT_ACCESS_FILE)
            ) =>
        {
            Ok(true)
        }
        Err(e) => Err(e),
    }
}

pub(crate) fn absolute(path: &Path) -> io::Result<PathBuf> {
    super::path::absolute(path.as_os_str())
}
