//! Open files: `File` and `OpenOptions`.

use core::{
    fmt,
    mem::{MaybeUninit, offset_of},
    ptr,
};

use windows_sys::{
    Wdk::Storage::FileSystem::{
        FILE_ALL_INFORMATION, FILE_BASIC_INFORMATION, FileAllInformation,
        NtQueryInformationFile,
    },
    Win32::{
        Foundation::{
            ERROR_ALREADY_EXISTS, ERROR_INVALID_FUNCTION,
            ERROR_INVALID_PARAMETER, ERROR_IO_PENDING, ERROR_LOCK_VIOLATION,
            ERROR_NOT_LOCKED, ERROR_NOT_SUPPORTED, FILETIME, GENERIC_READ,
            GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
            STATUS_BUFFER_OVERFLOW,
        },
        Storage::FileSystem::{
            CREATE_NEW, CreateFileW, FILE_ALLOCATION_INFO,
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
            FILE_BASIC_INFO, FILE_BEGIN, FILE_CURRENT,
            FILE_DISPOSITION_FLAG_DELETE,
            FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
            FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO,
            FILE_DISPOSITION_INFO_EX, FILE_END, FILE_END_OF_FILE_INFO,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_OVERLAPPED,
            FILE_GENERIC_WRITE, FILE_INFO_BY_HANDLE_CLASS, FILE_SHARE_DELETE,
            FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_DATA,
            FileAllocationInfo, FileAttributeTagInfo, FileBasicInfo,
            FileDispositionInfo, FileDispositionInfoEx, FileEndOfFileInfo,
            FlushFileBuffers, GetFileInformationByHandleEx,
            GetFinalPathNameByHandleW, LOCK_FILE_FLAGS,
            LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx,
            OPEN_ALWAYS, OPEN_EXISTING, SECURITY_SQOS_PRESENT,
            SetFileInformationByHandle, SetFilePointerEx, SetFileTime,
            TRUNCATE_EXISTING, UnlockFile, VOLUME_NAME_DOS,
        },
        System::{
            IO::{GetOverlappedResult, IO_STATUS_BLOCK, OVERLAPPED},
            Threading::CreateEventW,
        },
    },
};

use super::{
    fs::{FileAttr, FilePermissions, FileTimes, filetime, signed, unsigned},
    os::{
        cvt,
        handle::{self, Transfer, stream_io},
        last_error, nt_error, os_code,
        wide::fill_os_string,
        win_error,
    },
    path::NativePath,
};
use crate::{
    fs::TryLockError,
    io::{self, IoSlice, IoSliceMut, SeekFrom},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::{Path, PathBuf},
};

/// Sets the `class` information of `handle`, returning the error code.
///
/// # Safety
///
/// `T` must be the structure that `class` takes, and `handle` must be open.
pub(crate) unsafe fn set_file_info<T>(
    handle: HANDLE,
    class: FILE_INFO_BY_HANDLE_CLASS,
    info: &T,
) -> Result<(), u32> {
    // The structures are a few bytes long.
    #[allow(clippy::cast_possible_truncation)]
    let size = size_of::<T>() as u32;
    // SAFETY: `info` is a `T` of `size` bytes, which is what `class` takes,
    // and the caller guarantees that `handle` is open.
    let ok = unsafe {
        SetFileInformationByHandle(
            handle,
            class,
            ptr::from_ref(info).cast(),
            size,
        )
    };
    if ok == 0 { Err(last_error()) } else { Ok(()) }
}

/// Marks the file for POSIX deletion, which removes its name when `handle`
/// closes even while other handles stay open, ignoring the read-only
/// attribute. File systems without it, such as FAT, fail with
/// `ERROR_INVALID_PARAMETER`, `ERROR_NOT_SUPPORTED` or
/// `ERROR_INVALID_FUNCTION`.
pub(crate) fn posix_delete(handle: HANDLE) -> Result<(), u32> {
    let info = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    // SAFETY: the class takes this structure, and `handle` is open.
    unsafe { set_file_info(handle, FileDispositionInfoEx, &info) }
}

/// Marks the file for deletion: POSIX semantics where supported, else Win32
/// semantics, which delete the file once every handle to it closed.
pub(crate) fn delete(handle: HANDLE) -> Result<(), u32> {
    match posix_delete(handle) {
        Err(
            ERROR_INVALID_PARAMETER
            | ERROR_NOT_SUPPORTED
            | ERROR_INVALID_FUNCTION,
        ) => {
            let info = FILE_DISPOSITION_INFO { DeleteFile: true };
            // SAFETY: the class takes this structure, and `handle` is open.
            unsafe { set_file_info(handle, FileDispositionInfo, &info) }
        }
        result => result,
    }
}

/// Options and flags which can be used to configure how a file is opened.
#[derive(Clone)]
#[allow(clippy::struct_excessive_bools, reason = "std's options, one each")]
pub(crate) struct OpenOptions {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
    custom_flags: u32,
    access_mode: Option<u32>,
    attributes: u32,
    share_mode: u32,
    security_qos_flags: u32,
}

impl fmt::Debug for OpenOptions {
    /// Prints std's fields; those for handle inheritance and std's unstable
    /// options, which litestd leaves out, are always `false`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenOptions")
            .field("read", &self.read)
            .field("write", &self.write)
            .field("append", &self.append)
            .field("truncate", &self.truncate)
            .field("create", &self.create)
            .field("create_new", &self.create_new)
            .field("custom_flags", &self.custom_flags)
            .field("access_mode", &self.access_mode)
            .field("attributes", &self.attributes)
            .field("share_mode", &self.share_mode)
            .field("security_qos_flags", &self.security_qos_flags)
            .field("inherit_handle", &false)
            .field("freeze_last_access_time", &false)
            .field("freeze_last_write_time", &false)
            .finish()
    }
}

const NEEDS_WRITE: io::Error = io::const_error!(
    io::ErrorKind::InvalidInput,
    "creating or truncating a file requires write or append access",
);
const APPEND_AND_TRUNCATE: io::Error = io::const_error!(
    io::ErrorKind::InvalidInput,
    "append and truncate cannot both be enabled",
);
const NEEDS_ACCESS: io::Error = io::const_error!(
    io::ErrorKind::InvalidInput,
    "must specify at least one of read, write, or append access",
);

/// Defines the setters of `OpenOptions` that only store their argument.
macro_rules! setters {
    ($($name:ident: $ty:ty),* $(,)?) => {$(
        pub(crate) const fn $name(&mut self, $name: $ty) {
            self.$name = $name;
        }
    )*};
}

impl OpenOptions {
    pub(crate) const fn new() -> Self {
        Self {
            read: false,
            write: false,
            append: false,
            truncate: false,
            create: false,
            create_new: false,
            custom_flags: 0,
            access_mode: None,
            attributes: 0,
            share_mode: FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            security_qos_flags: 0,
        }
    }

    setters! {
        read: bool, write: bool, append: bool, truncate: bool,
        create: bool, create_new: bool,
        custom_flags: u32, share_mode: u32, attributes: u32,
    }

    pub(crate) const fn access_mode(&mut self, access_mode: u32) {
        self.access_mode = Some(access_mode);
    }

    pub(crate) const fn security_qos_flags(&mut self, flags: u32) {
        // `SECURITY_ANONYMOUS` is 0, so mark the flags as present.
        self.security_qos_flags = flags | SECURITY_SQOS_PRESENT;
    }

    /// Returns the creation disposition, after checking the combination of
    /// options, which std does before it checks the access.
    const fn creation(&self) -> io::Result<u32> {
        match (self.write, self.append) {
            (true, false) => {}
            (false, false) => {
                if self.truncate || self.create || self.create_new {
                    return Err(NEEDS_WRITE);
                }
            }
            (_, true) => {
                if self.truncate && !self.create_new {
                    return Err(APPEND_AND_TRUNCATE);
                }
            }
        }
        Ok(match (self.create, self.truncate, self.create_new) {
            (false, false, false) => OPEN_EXISTING,
            // `CREATE_ALWAYS` would also reset the attributes and remove the
            // alternate data streams, so `File::open` truncates instead.
            (true, _, false) => OPEN_ALWAYS,
            (false, true, false) => TRUNCATE_EXISTING,
            (_, _, true) => CREATE_NEW,
        })
    }

    /// Returns the desired access.
    const fn access(&self) -> io::Result<u32> {
        /// Appending needs every write right but the one to overwrite.
        const APPEND: u32 = FILE_GENERIC_WRITE & !FILE_WRITE_DATA;
        match (self.read, self.write, self.append, self.access_mode) {
            (.., Some(mode)) => Ok(mode),
            (true, false, false, None) => Ok(GENERIC_READ),
            (false, true, false, None) => Ok(GENERIC_WRITE),
            (true, true, false, None) => Ok(GENERIC_READ | GENERIC_WRITE),
            (false, _, true, None) => Ok(APPEND),
            (true, _, true, None) => Ok(GENERIC_READ | APPEND),
            (false, false, false, None) => {
                if self.create || self.create_new || self.truncate {
                    Err(NEEDS_WRITE)
                } else {
                    Err(NEEDS_ACCESS)
                }
            }
        }
    }

    /// Returns the flags and attributes for `CreateFileW`.
    const fn flags(&self) -> u32 {
        // A new file must not be created through a dangling link.
        let no_follow = if self.create_new {
            FILE_FLAG_OPEN_REPARSE_POINT
        } else {
            0
        };
        self.custom_flags
            | self.attributes
            | self.security_qos_flags
            | no_follow
    }
}

/// An open file.
pub(crate) struct File {
    handle: OwnedHandle,
    /// Whether litestd opened the handle without `FILE_FLAG_OVERLAPPED`; a
    /// handle from elsewhere may do overlapped I/O.
    synchronous: bool,
}

impl File {
    /// Opens the file at `path` with the options specified by `opts`.
    pub(crate) fn open(path: &Path, opts: &OpenOptions) -> io::Result<Self> {
        let mut native = NativePath::new();
        Self::open_native(native.convert(path.as_os_str())?, opts)
    }

    /// Opens the existing file or directory at the converted,
    /// NUL-terminated `path`, with the given access rights and `CreateFileW`
    /// flags.
    pub(crate) fn open_existing(
        path: &[u16],
        access: u32,
        flags: u32,
    ) -> io::Result<Self> {
        let opts = OpenOptions {
            access_mode: Some(access),
            custom_flags: flags,
            ..OpenOptions::new()
        };
        Self::open_native(path, &opts)
    }

    /// Opens the file at the converted, NUL-terminated `path`.
    fn open_native(path: &[u16], opts: &OpenOptions) -> io::Result<Self> {
        let creation = opts.creation()?;
        let access = opts.access()?;
        let flags = opts.flags();
        // SAFETY: `path` is NUL-terminated; no security attributes or template.
        let raw = unsafe {
            CreateFileW(
                path.as_ptr(),
                access,
                opts.share_mode,
                ptr::null(),
                creation,
                flags,
                ptr::null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // On success, `OPEN_ALWAYS` reports whether the file existed.
        let existed =
            creation == OPEN_ALWAYS && last_error() == ERROR_ALREADY_EXISTS;
        let file = Self {
            // SAFETY: `CreateFileW` returned a new, unowned handle.
            handle: unsafe { OwnedHandle::from_raw_handle(raw) },
            synchronous: flags & FILE_FLAG_OVERLAPPED == 0,
        };
        if opts.truncate && existed {
            file.truncate_existing()?;
        }
        Ok(file)
    }

    /// Wraps a handle from elsewhere, which may do overlapped I/O.
    pub(crate) const fn from_handle(handle: OwnedHandle) -> Self {
        Self {
            handle,
            synchronous: false,
        }
    }

    pub(crate) const fn handle(&self) -> &OwnedHandle {
        &self.handle
    }

    pub(crate) fn into_handle(self) -> OwnedHandle {
        self.handle
    }

    fn raw(&self) -> HANDLE {
        self.handle.as_raw_handle()
    }

    /// Empties the file that `open` with `create` and `truncate` found.
    #[cold]
    fn truncate_existing(&self) -> io::Result<()> {
        let alloc = FILE_ALLOCATION_INFO { AllocationSize: 0 };
        let eof = FILE_END_OF_FILE_INFO { EndOfFile: 0 };
        // SAFETY: each class takes the structure passed with it, and the
        // handle is open. Wine lacks `FileAllocationInfo`, as in std.
        unsafe {
            set_file_info(self.raw(), FileAllocationInfo, &alloc)
                .or_else(|_| set_file_info(self.raw(), FileEndOfFileInfo, &eof))
        }
        .map_err(win_error)
    }

    pub(crate) fn file_attr(&self) -> io::Result<FileAttr> {
        // `GetFileInformationByHandle` would also query the volume, which
        // std's stable API does not need. The name at the end does not fit,
        // so the query fills the rest and fails with `STATUS_BUFFER_OVERFLOW`.
        let mut info = AllInformation::new();
        let mut status_block = IO_STATUS_BLOCK::default();
        #[allow(clippy::cast_possible_truncation)]
        let size = size_of::<AllInformation>() as u32;
        // SAFETY: `info` (`FILE_ALL_INFORMATION`'s layout, integers only) and
        // `status_block` are valid for writes, and the handle is open.
        let status = unsafe {
            NtQueryInformationFile(
                self.raw(),
                &raw mut status_block,
                (&raw mut info).cast(),
                size,
                FileAllInformation,
            )
        };
        if status < 0 && status != STATUS_BUFFER_OVERFLOW {
            return Err(nt_error(status));
        }
        let basic = info.basic;
        let reparse_tag =
            if basic.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
                0
            } else {
                self.reparse_tag()?
            };
        Ok(FileAttr {
            attributes: basic.FileAttributes,
            reparse_tag,
            creation_time: unsigned(basic.CreationTime),
            last_access_time: unsigned(basic.LastAccessTime),
            last_write_time: unsigned(basic.LastWriteTime),
            file_size: unsigned(info.end_of_file),
        })
    }

    /// Returns the reparse tag of the file, which is a reparse point.
    #[cold]
    fn reparse_tag(&self) -> io::Result<u32> {
        let mut tag = FILE_ATTRIBUTE_TAG_INFO::default();
        #[allow(clippy::cast_possible_truncation)]
        let size = size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32;
        // SAFETY: `tag` is writable for `size` bytes; the handle is open.
        cvt(unsafe {
            GetFileInformationByHandleEx(
                self.raw(),
                FileAttributeTagInfo,
                (&raw mut tag).cast(),
                size,
            )
        })?;
        Ok(if tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
            0
        } else {
            tag.ReparseTag
        })
    }

    pub(crate) fn fsync(&self) -> io::Result<()> {
        // SAFETY: the handle is open.
        cvt(unsafe { FlushFileBuffers(self.raw()) })
    }

    pub(crate) fn datasync(&self) -> io::Result<()> {
        self.fsync()
    }

    pub(crate) fn lock(&self) -> io::Result<()> {
        self.lock_file(LOCKFILE_EXCLUSIVE_LOCK)
    }

    pub(crate) fn lock_shared(&self) -> io::Result<()> {
        self.lock_file(0)
    }

    pub(crate) fn try_lock(&self) -> Result<(), TryLockError> {
        try_lock_result(
            self.lock_file(LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY),
        )
    }

    pub(crate) fn try_lock_shared(&self) -> Result<(), TryLockError> {
        try_lock_result(self.lock_file(LOCKFILE_FAIL_IMMEDIATELY))
    }

    /// Locks the whole file, as far as it can ever extend.
    fn lock_file(&self, flags: LOCK_FILE_FLAGS) -> io::Result<()> {
        if !self.synchronous {
            return self.lock_overlapped(flags);
        }
        let mut overlapped = OVERLAPPED::default();
        // SAFETY: the handle is open and synchronous, so the lock completes
        // before the call returns and `overlapped` is not used afterwards.
        cvt(unsafe {
            LockFileEx(
                self.raw(),
                flags,
                0,
                u32::MAX,
                u32::MAX,
                &raw mut overlapped,
            )
        })
    }

    /// Locks through a handle that may do overlapped I/O, on which the lock
    /// can be granted after `LockFileEx` returned.
    #[cold]
    fn lock_overlapped(&self, flags: LOCK_FILE_FLAGS) -> io::Result<()> {
        // SAFETY: an unnamed auto-reset event without security attributes.
        let event = unsafe { CreateEventW(ptr::null(), 0, 0, ptr::null()) };
        if event.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `CreateEventW` returned a new, unowned handle.
        let event = unsafe { OwnedHandle::from_raw_handle(event) };
        let mut overlapped = OVERLAPPED {
            // Only this lock signals the event, so the wait below ends when
            // the lock completes. The low bit keeps the completion off any
            // I/O completion port, whose packet would point into this frame.
            hEvent: event.as_raw_handle().map_addr(|addr| addr | 1),
            ..OVERLAPPED::default()
        };
        // SAFETY: the handle is open, and `overlapped` outlives the lock: if
        // it is pending, `GetOverlappedResult` waits for it.
        let ok = unsafe {
            LockFileEx(
                self.raw(),
                flags,
                0,
                u32::MAX,
                u32::MAX,
                &raw mut overlapped,
            )
        };
        if ok != 0 {
            return Ok(());
        }
        let error = last_error();
        if error != ERROR_IO_PENDING {
            return Err(win_error(error));
        }
        let mut transferred = 0;
        // SAFETY: `overlapped` is the pending lock's; the call waits for it.
        cvt(unsafe {
            GetOverlappedResult(
                self.raw(),
                &raw const overlapped,
                &raw mut transferred,
                1,
            )
        })
    }

    pub(crate) fn unlock(&self) -> io::Result<()> {
        // A handle can hold both an exclusive and a shared lock on the
        // region, which then takes two unlocks.
        self.unlock_once()?;
        match self.unlock_once() {
            Err(e) if e.raw_os_error() == Some(os_code(ERROR_NOT_LOCKED)) => {
                Ok(())
            }
            result => result,
        }
    }

    fn unlock_once(&self) -> io::Result<()> {
        // SAFETY: the handle is open.
        cvt(unsafe { UnlockFile(self.raw(), 0, 0, u32::MAX, u32::MAX) })
    }

    pub(crate) fn truncate(&self, size: u64) -> io::Result<()> {
        // Sizes above `i64::MAX` turn negative and fail, as in std.
        let info = FILE_END_OF_FILE_INFO {
            EndOfFile: signed(size),
        };
        // SAFETY: the class takes this structure, and the handle is open.
        unsafe { set_file_info(self.raw(), FileEndOfFileInfo, &info) }
            .map_err(win_error)
    }

    /// Reads into `buf`, which only the kernel writes. A handle from
    /// elsewhere may do overlapped I/O; `handle::read` waits for it.
    pub(crate) fn read_uninit(
        &self,
        buf: &mut [MaybeUninit<u8>],
    ) -> io::Result<usize> {
        handle::read(self.raw(), buf)
    }

    pub(crate) fn read_at(
        &self,
        buf: &mut [u8],
        offset: u64,
    ) -> io::Result<usize> {
        // SAFETY: the transfer lends the buffer to the kernel alone.
        let buf = unsafe { io::as_uninit(buf) };
        handle::transfer(self.raw(), Transfer::Read(buf), Some(offset))
            .map_err(io::Error::from_raw_os_error)
    }

    pub(crate) fn write(&self, buf: &[u8]) -> io::Result<usize> {
        handle::write(self.raw(), buf)
    }

    pub(crate) fn write_at(
        &self,
        buf: &[u8],
        offset: u64,
    ) -> io::Result<usize> {
        handle::transfer(self.raw(), Transfer::Write(buf), Some(offset))
            .map_err(io::Error::from_raw_os_error)
    }

    stream_io!();

    #[allow(clippy::unused_self, clippy::unnecessary_wraps)]
    pub(crate) const fn flush(&self) -> io::Result<()> {
        Ok(())
    }

    pub(crate) fn seek(&self, pos: SeekFrom) -> io::Result<u64> {
        let (method, distance) = match pos {
            // The OS reads a distance from the start as unsigned.
            SeekFrom::Start(n) => (FILE_BEGIN, signed(n)),
            SeekFrom::End(n) => (FILE_END, n),
            SeekFrom::Current(n) => (FILE_CURRENT, n),
        };
        let mut position = 0;
        // SAFETY: `position` is valid for writes, and the handle is open.
        cvt(unsafe {
            SetFilePointerEx(self.raw(), distance, &raw mut position, method)
        })?;
        Ok(unsigned(position))
    }

    pub(crate) fn tell(&self) -> io::Result<u64> {
        self.seek(SeekFrom::Current(0))
    }

    pub(crate) fn duplicate(&self) -> io::Result<Self> {
        Ok(Self {
            handle: self.handle.try_clone()?,
            synchronous: self.synchronous,
        })
    }

    pub(crate) fn set_permissions(
        &self,
        perm: FilePermissions,
    ) -> io::Result<()> {
        // Zero times and zero attributes leave them unchanged, as in std.
        let info = FILE_BASIC_INFO {
            FileAttributes: perm.attributes(),
            ..FILE_BASIC_INFO::default()
        };
        // SAFETY: the class takes this structure, and the handle is open.
        unsafe { set_file_info(self.raw(), FileBasicInfo, &info) }
            .map_err(win_error)
    }

    pub(crate) fn set_times(&self, times: FileTimes) -> io::Result<()> {
        let mut values = [None; 3];
        let requested = [times.created, times.accessed, times.modified];
        for (value, time) in values.iter_mut().zip(requested) {
            *value = time.map(filetime).transpose()?;
        }
        // `SetFileTime` reads 0 as "leave unchanged". All ones, which it
        // reads as "stop updating", is out of the range `filetime` checks.
        if values.contains(&Some(0)) {
            return Err(io::const_error!(
                io::ErrorKind::InvalidInput,
                "cannot set file timestamp to 0",
            ));
        }
        let [created, accessed, modified] =
            values.map(|value| value.map(split_filetime));
        let pointer = |t: &Option<FILETIME>| {
            t.as_ref().map_or(ptr::null(), ptr::from_ref)
        };
        // SAFETY: each pointer is null or points to a `FILETIME` that
        // outlives the call, and the handle is open.
        cvt(unsafe {
            SetFileTime(
                self.raw(),
                pointer(&created),
                pointer(&accessed),
                pointer(&modified),
            )
        })
    }
}

/// `FILE_ALL_INFORMATION` with plain bytes where windows-sys declares the
/// flags of its standard part as `bool`: the kernel may store any byte in
/// them. Only the fields that `file_attr` reads are named.
#[repr(C)]
struct AllInformation {
    basic: FILE_BASIC_INFORMATION,
    _allocation_size: i64,
    end_of_file: i64,
    _rest: [u8; ALL_INFORMATION_REST],
}

/// The bytes of `FILE_ALL_INFORMATION` after its `EndOfFile`.
const ALL_INFORMATION_REST: usize = size_of::<FILE_ALL_INFORMATION>()
    - offset_of!(FILE_ALL_INFORMATION, StandardInformation.NumberOfLinks);

// Same size as `FILE_ALL_INFORMATION`, with the named fields in place.
const _: () = {
    assert!(size_of::<AllInformation>() == size_of::<FILE_ALL_INFORMATION>());
    assert!(
        offset_of!(AllInformation, basic)
            == offset_of!(FILE_ALL_INFORMATION, BasicInformation)
    );
    assert!(
        offset_of!(AllInformation, end_of_file)
            == offset_of!(FILE_ALL_INFORMATION, StandardInformation.EndOfFile)
    );
};

impl AllInformation {
    fn new() -> Self {
        Self {
            basic: FILE_BASIC_INFORMATION::default(),
            _allocation_size: 0,
            end_of_file: 0,
            _rest: [0; ALL_INFORMATION_REST],
        }
    }
}

/// Splits a 64-bit time into a `FILETIME`.
const fn split_filetime(t: u64) -> FILETIME {
    // The casts keep the low and the high 32 bits.
    #[allow(clippy::cast_possible_truncation)]
    FILETIME {
        dwLowDateTime: t as u32,
        dwHighDateTime: (t >> 32) as u32,
    }
}

/// Maps the result of a lock attempt that does not block.
fn try_lock_result(result: io::Result<()>) -> Result<(), TryLockError> {
    match result {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(os_code(ERROR_LOCK_VIOLATION)) => {
            Err(TryLockError::WouldBlock)
        }
        Err(e) => Err(TryLockError::Error(e)),
    }
}

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut b = f.debug_struct("File");
        b.field("handle", &self.raw());
        if let Ok(path) = final_path(self.raw()) {
            b.field("path", &path);
        }
        b.finish()
    }
}

/// Returns the path of the file behind `handle`, in its verbatim form.
pub(crate) fn final_path(handle: HANDLE) -> io::Result<PathBuf> {
    fill_os_string(&mut |ptr, size| {
        // SAFETY: `ptr` is valid for writes of `size` units; `handle` is open.
        unsafe { GetFinalPathNameByHandleW(handle, ptr, size, VOLUME_NAME_DOS) }
    })
    .map(PathBuf::from)
}
