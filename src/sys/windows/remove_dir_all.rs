//! `remove_dir_all`, immune to symbolic link races (CVE-2022-21658).
//!
//! No path below the root is ever resolved: every entry is opened with
//! `NtOpenFile` relative to a handle to its parent directory, as the reparse
//! point itself if it is one, so only what is inside the directories held
//! open is listed and deleted, and a link in the tree is deleted, never
//! followed. The walk is depth first over an explicit stack of directory
//! handles. Deletions retry briefly, as in std, while a file is in use or
//! Win32 semantics keep deleted names until their handles close; an entry
//! that vanishes counts as deleted.

use core::{
    ptr,
    sync::atomic::{AtomicU32, Ordering},
};

use alloc_crate::vec::Vec;
use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT, NtOpenFile,
        },
    },
    Win32::{
        Foundation::{
            ERROR_BAD_NET_NAME, ERROR_BAD_NETPATH, ERROR_DELETE_PENDING,
            ERROR_DIR_NOT_EMPTY, ERROR_DIRECTORY, ERROR_FILE_NOT_FOUND,
            ERROR_FILENAME_EXCED_RANGE, ERROR_INVALID_PARAMETER,
            ERROR_NO_MORE_FILES, ERROR_PATH_NOT_FOUND, ERROR_SHARING_VIOLATION,
            HANDLE, OBJ_DONT_REPARSE, RtlNtStatusToDosError,
            STATUS_DELETE_PENDING, UNICODE_STRING,
        },
        Storage::FileSystem::{
            DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_BASIC_INFO,
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_FULL_DIR_INFO, FILE_LIST_DIRECTORY, FILE_SHARE_DELETE,
            FILE_SHARE_READ, FILE_SHARE_WRITE, FileBasicInfo,
            FileFullDirectoryInfo, FileFullDirectoryRestartInfo,
            GetFileInformationByHandleEx, SYNCHRONIZE,
        },
        System::{IO::IO_STATUS_BLOCK, Threading::SwitchToThread},
    },
};

use super::{
    file::{File, delete},
    os::{cvt, last_error, win_error},
    path::NativePath,
};
use crate::{
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::Path,
};

/// The deepest directory the walk enters, counting the root as 1. A Win32
/// path reaches at most 16,383 levels, and the walk holds one handle per
/// level; a deeper tree, built by moving directories, fails with
/// `ERROR_FILENAME_EXCED_RANGE`, where std would go on.
const MAX_DEPTH: usize = 16_384;

/// Attempts at a deletion that other handles to the file block, as in std.
const MAX_ATTEMPTS: usize = 50;

/// Bytes of directory entries fetched per call. The walk lists a parent
/// again after each subdirectory, so larger buffers cost more there.
const BUFFER_BYTES: u32 = 4096;

/// Directory entries as `fill` fetches them, aligned for their 64-bit
/// fields as the kernel requires.
#[repr(C, align(8))]
struct EntryBuffer([u8; BUFFER_BYTES as usize]);

pub(crate) fn remove_dir_all(path: &Path) -> io::Result<()> {
    // The root is opened as the reparse point itself too: a link to a
    // directory is removed, not what it points to.
    let flags = FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT;
    // The converted path is not needed during the walk.
    let root = {
        let mut native = NativePath::new();
        let path = native.convert(path.as_os_str())?;
        File::open_existing(path, FILE_LIST_DIRECTORY, flags)?
    };
    let mut info = FILE_BASIC_INFO::default();
    #[allow(clippy::cast_possible_truncation)]
    let size = size_of::<FILE_BASIC_INFO>() as u32;
    // SAFETY: `info` is valid for writes of `size` bytes; the handle is open.
    cvt(unsafe {
        GetFileInformationByHandleEx(
            root.handle().as_raw_handle(),
            FileBasicInfo,
            (&raw mut info).cast(),
            size,
        )
    })?;
    if info.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(win_error(ERROR_DIRECTORY));
    }
    remove_tree(root.into_handle()).map_err(win_error)
}

/// Deletes the directory `root` and everything in it.
fn remove_tree(root: OwnedHandle) -> Result<(), u32> {
    let mut buffer = EntryBuffer([0; BUFFER_BYTES as usize]);
    let mut name = Vec::new();
    let mut stack = Vec::with_capacity(16);
    stack.push(root);
    let mut restart = true;
    // Each pass lists more of the top directory, enters a subdirectory, or
    // deletes the emptied top. Depth is bounded by `MAX_DEPTH` and listing by
    // the tree, which shrinks with every deletion; the loop ends when the
    // root is deleted, unless other processes refill the tree as fast.
    while let Some(top) = stack.last() {
        let dir = top.as_raw_handle();
        if !fill(dir, &mut buffer, restart)? {
            // Empty. Its handle closes at the end of this block, completing
            // the deletion before the parent is listed again.
            let emptied = stack.pop();
            if stack.is_empty() {
                // The root was opened without the right to delete: that
                // would fail before its contents are deleted if another
                // process has it open without sharing deletion.
                retry(ERROR_DIR_NOT_EMPTY, &mut || delete_child(dir, &[]))?;
            } else {
                // Subdirectories were opened with the right to delete.
                retry(ERROR_DIR_NOT_EMPTY, &mut || delete(dir))?;
            }
            drop(emptied);
            // The walk left the parent's listing; list it again.
            restart = true;
            continue;
        }
        restart = false;
        let mut subdir = None;
        for (entry, is_directory) in Entries::new(&buffer.0) {
            decode_name(entry, &mut name);
            if is_directory {
                subdir = open_child(
                    dir,
                    &name,
                    DIRECTORY_ACCESS,
                    FILE_SYNCHRONOUS_IO_NONALERT,
                )?;
                if subdir.is_some() {
                    break;
                }
            } else {
                retry(ERROR_SHARING_VIOLATION, &mut || {
                    delete_child(dir, &name)
                })?;
            }
        }
        if let Some(subdir) = subdir {
            if stack.len() >= MAX_DEPTH {
                return Err(ERROR_FILENAME_EXCED_RANGE);
            }
            stack.push(subdir);
        }
    }
    Ok(())
}

/// The access to directories in the tree: to list them, to wait on them for
/// synchronous listing, and to delete them.
const DIRECTORY_ACCESS: u32 = FILE_LIST_DIRECTORY | SYNCHRONIZE | DELETE;

/// Fills `buffer` with the next entries of the directory `dir`, from the
/// start if `restart`. Returns `false` once there are no more.
fn fill(
    dir: HANDLE,
    buffer: &mut EntryBuffer,
    restart: bool,
) -> Result<bool, u32> {
    let class = if restart {
        FileFullDirectoryRestartInfo
    } else {
        FileFullDirectoryInfo
    };
    // SAFETY: `buffer` is valid for writes of `BUFFER_BYTES` bytes and
    // aligned as the entries need; `dir` is an open directory handle for
    // synchronous I/O, so the call completes before it returns.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            dir,
            class,
            buffer.0.as_mut_ptr().cast(),
            BUFFER_BYTES,
        )
    };
    if ok != 0 {
        return Ok(true);
    }
    match last_error() {
        ERROR_NO_MORE_FILES => Ok(false),
        error => Err(error),
    }
}

/// An iterator over the entries `fill` fetched: each name, as UTF-16 bytes,
/// and whether it is a directory, skipping `.` and `..`. Fields are read
/// within the buffer whatever the file system reports; an entry that does
/// not fit ends the iteration.
struct Entries<'a> {
    bytes: &'a [u8],
    /// The offset of the next entry, or `None` after the last.
    next: Option<usize>,
}

impl<'a> Entries<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            next: Some(0),
        }
    }

    fn u32_at(&self, offset: usize) -> Option<u32> {
        let bytes = self.bytes.get(offset..)?.first_chunk().copied();
        Some(u32::from_ne_bytes(bytes?))
    }

    /// Reads the entry at `offset`: its name, whether it is a directory,
    /// and the offset of the entry after it, relative to this one.
    fn entry(&self, offset: usize) -> Option<(&'a [u8], bool, usize)> {
        use core::mem::offset_of;
        // Below the buffer size, so the header offsets cannot overflow.
        if offset >= self.bytes.len() {
            return None;
        }
        let next = self
            .u32_at(offset + offset_of!(FILE_FULL_DIR_INFO, NextEntryOffset))?;
        let attributes = self
            .u32_at(offset + offset_of!(FILE_FULL_DIR_INFO, FileAttributes))?;
        let name_len = self
            .u32_at(offset + offset_of!(FILE_FULL_DIR_INFO, FileNameLength))?;
        let start = offset + offset_of!(FILE_FULL_DIR_INFO, FileName);
        let end = start.checked_add(usize::try_from(name_len).ok()?)?;
        let name = self.bytes.get(start..end)?;
        Some((
            name,
            attributes & FILE_ATTRIBUTE_DIRECTORY != 0,
            usize::try_from(next).ok()?,
        ))
    }
}

impl<'a> Iterator for Entries<'a> {
    type Item = (&'a [u8], bool);

    fn next(&mut self) -> Option<Self::Item> {
        // Each pass moves `next` forward by a nonzero offset or ends the
        // iteration, and offsets past the buffer end it, so the loop runs
        // at most once per byte of the buffer.
        loop {
            let offset = self.next.take()?;
            let (name, is_directory, next) = self.entry(offset)?;
            if next != 0 {
                self.next = offset.checked_add(next);
            }
            // `.` and `..`, and empty names, which no file has.
            if !matches!(name, [] | [b'.', 0] | [b'.', 0, b'.', 0]) {
                return Some((name, is_directory));
            }
        }
    }
}

/// Decodes the UTF-16 bytes of a name into `name`, aligned for the kernel.
fn decode_name(bytes: &[u8], name: &mut Vec<u16>) {
    name.clear();
    name.extend(
        bytes
            .chunks_exact(2)
            .filter_map(|unit| <[u8; 2]>::try_from(unit).ok())
            .map(u16::from_ne_bytes),
    );
}

/// Opens the entry `name` of the directory `parent`, as the reparse point
/// itself if it is one, requesting `access`. Returns `None` if the entry
/// is gone or being deleted.
fn open_child(
    parent: HANDLE,
    name: &[u16],
    access: u32,
    options: u32,
) -> Result<Option<OwnedHandle>, u32> {
    /// Forbids following any reparse point while parsing the name; cleared
    /// on first use by older Windows 10 versions, which reject the flag.
    static ATTRIBUTES: AtomicU32 = AtomicU32::new(OBJ_DONT_REPARSE);
    /// The empty name, NUL-terminated for file systems that expect it.
    static EMPTY: [u16; 1] = [0];
    /// The size of the object attributes, which the kernel checks.
    #[allow(clippy::cast_possible_truncation)]
    const OBJECT_ATTRIBUTES_LEN: u32 = size_of::<OBJECT_ATTRIBUTES>() as u32;

    let len = u16::try_from(size_of_val(name))
        .map_err(|_| ERROR_FILENAME_EXCED_RANGE)?;
    let object_name = UNICODE_STRING {
        Length: len,
        MaximumLength: if name.is_empty() { 2 } else { len },
        // The kernel only reads the name.
        Buffer: if name.is_empty() {
            EMPTY.as_ptr()
        } else {
            name.as_ptr()
        }
        .cast_mut(),
    };
    // The flag only saves a retry, so `Relaxed` suffices.
    let mut attributes = ATTRIBUTES.load(Ordering::Relaxed);
    // At most two passes: a second one only without the flag.
    let error = loop {
        let object = OBJECT_ATTRIBUTES {
            Length: OBJECT_ATTRIBUTES_LEN,
            RootDirectory: parent,
            ObjectName: &raw const object_name,
            Attributes: attributes,
            SecurityDescriptor: ptr::null(),
            SecurityQualityOfService: ptr::null(),
        };
        let mut handle = ptr::null_mut();
        let mut status_block = IO_STATUS_BLOCK::default();
        // SAFETY: `object` names the open directory `parent` and a name
        // whose buffer holds its `Length` bytes and outlives the call;
        // `handle` and `status_block` are valid for writes. Without an
        // asynchronous option, the call completes before it returns.
        let status = unsafe {
            NtOpenFile(
                &raw mut handle,
                access,
                &raw const object,
                &raw mut status_block,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                FILE_OPEN_REPARSE_POINT | options,
            )
        };
        if status >= 0 {
            // SAFETY: `NtOpenFile` returned a new, unowned handle.
            return Ok(Some(unsafe { OwnedHandle::from_raw_handle(handle) }));
        }
        // The generic mapping reports a pending deletion as access denied,
        // which could also mean missing permissions.
        let error = if status == STATUS_DELETE_PENDING {
            ERROR_DELETE_PENDING
        } else {
            // SAFETY: `RtlNtStatusToDosError` has no preconditions.
            unsafe { RtlNtStatusToDosError(status) }
        };
        if error != ERROR_INVALID_PARAMETER || attributes == 0 {
            break error;
        }
        ATTRIBUTES.store(0, Ordering::Relaxed);
        attributes = 0;
    };
    match error {
        // Gone, or being deleted by someone else, which will succeed.
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_BAD_NETPATH
        | ERROR_BAD_NET_NAME | ERROR_DELETE_PENDING => Ok(None),
        error => Err(error),
    }
}

/// Deletes the entry `name` of the directory `parent`, or `parent` itself
/// if `name` is empty.
fn delete_child(parent: HANDLE, name: &[u16]) -> Result<(), u32> {
    // Closing the handle at once completes a Win32 deletion soonest.
    open_child(parent, name, DELETE, 0)?
        .map_or(Ok(()), |file| delete(file.as_raw_handle()))
}

/// Runs `f` until it does not fail with `transient`, at most
/// `MAX_ATTEMPTS` times, yielding the processor between attempts.
fn retry(
    transient: u32,
    f: &mut dyn FnMut() -> Result<(), u32>,
) -> Result<(), u32> {
    for _ in 1..MAX_ATTEMPTS {
        match f() {
            Err(error) if error == transient => {
                // SAFETY: `SwitchToThread` has no preconditions; whether
                // another thread ran does not matter.
                unsafe { SwitchToThread() };
            }
            result => return result,
        }
    }
    f()
}
