//! File system operations on top of POSIX file descriptors, each opened
//! close-on-exec and owned by an `OwnedFd`.

mod attr;
#[cfg(not(target_vendor = "apple"))]
mod copy;
#[cfg(target_vendor = "apple")]
mod copyfile;
mod dir;
#[cfg(target_vendor = "apple")]
mod times;

use core::{
    ffi::{CStr, c_char, c_int, c_uint},
    fmt,
    mem::MaybeUninit,
};

#[cfg(not(any(
    all(target_os = "linux", target_env = "gnu"),
    target_os = "android"
)))]
use libc::{ftruncate as ftruncate64, lseek as lseek64};
#[cfg(any(
    all(target_os = "linux", target_env = "gnu"),
    target_os = "android"
))]
use libc::{ftruncate64, lseek64, pread64, pwrite64};
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
use libc::{open as open64, openat as openat64};
#[cfg(all(target_os = "linux", target_env = "gnu"))]
use libc::{open64, openat64};
#[cfg(not(any(
    all(target_os = "linux", target_env = "gnu"),
    target_os = "android",
    target_os = "wasi"
)))]
use libc::{pread as pread64, pwrite as pwrite64};

#[cfg(not(target_vendor = "apple"))]
pub(crate) use self::copy::copy;
#[cfg(target_vendor = "apple")]
pub(crate) use self::copyfile::copy;
pub(crate) use self::{
    attr::FileAttr,
    dir::{DirEntry, ReadDir, readdir, remove_dir_all},
};
use super::os::{
    self, STACK_BUF, cvt, cvt_r,
    path::{self as sys_path, path_cstr as cstr},
};
#[cfg(any(
    target_vendor = "apple",
    target_os = "freebsd",
    target_os = "netbsd"
))]
use crate::ffi::OsString;
use crate::{
    fs::TryLockError,
    io::{self, IoSlice, IoSliceMut, SeekFrom},
    os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
    path::{Path, PathBuf},
    time::SystemTime,
};

type PathCall = unsafe extern "C" fn(*const c_char) -> c_int;

type PathsCall = unsafe extern "C" fn(*const c_char, *const c_char) -> c_int;

fn call_path(path: &Path, call: PathCall) -> io::Result<()> {
    let mut buf = STACK_BUF;
    let path = cstr(path, &mut buf)?;
    // SAFETY: every `PathCall` requires only a valid C string.
    cvt(unsafe { call(path.as_ptr()) }).map(drop)
}

fn call_paths(a: &Path, b: &Path, call: PathsCall) -> io::Result<()> {
    let (mut buf_a, mut buf_b) = (STACK_BUF, STACK_BUF);
    let a = cstr(a, &mut buf_a)?;
    let b = cstr(b, &mut buf_b)?;
    // SAFETY: every `PathsCall` requires only two valid C strings.
    cvt(unsafe { call(a.as_ptr(), b.as_ptr()) }).map(drop)
}

/// Converts a mode to std's `u32`; `mode_t` is `u16` on macOS.
#[allow(clippy::unnecessary_cast, reason = "`mode_t` is `u16` on macOS")]
const fn mode_u32(mode: libc::mode_t) -> u32 {
    mode as u32
}

/// Converts std's `u32` mode to `mode_t`, which drops the high bits on
/// macOS, where `mode_t` is `u16`, as std does.
#[allow(
    clippy::unnecessary_cast,
    clippy::cast_possible_truncation,
    reason = "`mode_t` is `u16` on macOS"
)]
const fn to_mode(mode: u32) -> libc::mode_t {
    mode as libc::mode_t
}

/// Opens `path` close-on-exec with `flags`, and `mode` for a new file.
fn open_c(
    path: &CStr,
    flags: c_int,
    mode: libc::mode_t,
) -> io::Result<OwnedFd> {
    let flags = flags | libc::O_CLOEXEC;
    // A variadic argument, which C promotes to `unsigned int`.
    let mode: c_uint = mode_u32(mode);
    // SAFETY: `path` is a valid C string, and the mode has the promoted type.
    let fd = cvt_r(|| unsafe { open64(path.as_ptr(), flags, mode) })?;
    // SAFETY: `open` returned a new descriptor that nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Opens the directory `name` relative to `dirfd`, close-on-exec.
fn open_dir_at(dirfd: c_int, name: &CStr, flags: c_int) -> io::Result<OwnedFd> {
    let flags = flags | libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY;
    // SAFETY: `name` is a C string; without `O_CREAT`, no mode is read.
    let fd = cvt_r(|| unsafe { openat64(dirfd, name.as_ptr(), flags) })?;
    // SAFETY: `openat` returned a new descriptor that nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Converts a byte count that `cvt` has already checked for -1.
#[cfg(not(target_vendor = "apple"))]
const fn byte_count(n: isize) -> usize {
    n.unsigned_abs()
}

/// Converts a file offset for the kernel. Offsets past `i64::MAX` wrap to
/// negative values, which the kernel rejects with `EINVAL`, as in std.
#[allow(clippy::cast_possible_wrap)]
const fn file_offset(offset: u64) -> i64 {
    offset as i64
}

/// An open file, owning its descriptor.
pub(crate) struct File(OwnedFd);

impl File {
    pub(crate) fn open(path: &Path, opts: &OpenOptions) -> io::Result<Self> {
        let mut buf = STACK_BUF;
        let path = cstr(path, &mut buf)?;
        let flags = opts.access_mode()?
            | opts.creation_mode()?
            | (opts.custom_flags & !libc::O_ACCMODE);
        open_c(&path, flags, opts.mode).map(Self)
    }

    pub(crate) const fn from_fd(fd: OwnedFd) -> Self {
        Self(fd)
    }

    pub(crate) fn into_fd(self) -> OwnedFd {
        self.0
    }

    pub(crate) fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }

    fn fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }

    pub(crate) fn file_attr(&self) -> io::Result<FileAttr> {
        attr::fstat(self.fd())
    }

    /// Flushes the file's data and metadata to the device. On macOS, where
    /// `fsync` only hands them to the drive, std asks for `F_FULLFSYNC`,
    /// which waits until the drive stored them.
    pub(crate) fn fsync(&self) -> io::Result<()> {
        // SAFETY: `fsync` touches no memory.
        #[cfg(not(target_vendor = "apple"))]
        let r = cvt_r(|| unsafe { libc::fsync(self.fd()) });
        #[cfg(target_vendor = "apple")]
        let r = self.full_fsync();
        r.map(drop)
    }

    /// Flushes the file's data to the device, and on macOS, as std does,
    /// its metadata too: see [`File::fsync`].
    pub(crate) fn datasync(&self) -> io::Result<()> {
        // SAFETY: `fdatasync` touches no memory.
        #[cfg(not(target_vendor = "apple"))]
        let r = cvt_r(|| unsafe { libc::fdatasync(self.fd()) });
        #[cfg(target_vendor = "apple")]
        let r = self.full_fsync();
        r.map(drop)
    }

    #[cfg(target_vendor = "apple")]
    fn full_fsync(&self) -> io::Result<c_int> {
        // SAFETY: `F_FULLFSYNC` touches no memory.
        cvt_r(|| unsafe { libc::fcntl(self.fd(), libc::F_FULLFSYNC) })
    }

    /// Applies the `flock` operation `op`. Like std, a blocking lock
    /// interrupted by a signal fails with `Interrupted` rather than retrying.
    #[cfg(unix)]
    fn flock(&self, op: c_int) -> io::Result<()> {
        // SAFETY: `flock` touches no memory.
        cvt(unsafe { libc::flock(self.fd(), op) }).map(drop)
    }

    #[cfg(unix)]
    pub(crate) fn lock(&self) -> io::Result<()> {
        self.flock(libc::LOCK_EX)
    }

    #[cfg(unix)]
    pub(crate) fn lock_shared(&self) -> io::Result<()> {
        self.flock(libc::LOCK_SH)
    }

    #[cfg(unix)]
    pub(crate) fn try_lock(&self) -> Result<(), TryLockError> {
        self.try_flock(libc::LOCK_EX)
    }

    #[cfg(unix)]
    pub(crate) fn try_lock_shared(&self) -> Result<(), TryLockError> {
        self.try_flock(libc::LOCK_SH)
    }

    #[cfg(unix)]
    fn try_flock(&self, op: c_int) -> Result<(), TryLockError> {
        match self.flock(op | libc::LOCK_NB) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                Err(TryLockError::WouldBlock)
            }
            Err(e) => Err(TryLockError::Error(e)),
        }
    }

    #[cfg(unix)]
    pub(crate) fn unlock(&self) -> io::Result<()> {
        self.flock(libc::LOCK_UN)
    }

    // WASI has no file locks; these fail with std's errors.

    #[cfg(target_os = "wasi")]
    #[allow(clippy::unused_self, reason = "std's signature")]
    #[allow(clippy::missing_const_for_fn, reason = "stands in for an OS call")]
    pub(crate) fn lock(&self) -> io::Result<()> {
        Err(io::const_error!(
            io::ErrorKind::Unsupported,
            "lock() not supported"
        ))
    }

    #[cfg(target_os = "wasi")]
    #[allow(clippy::unused_self, reason = "std's signature")]
    #[allow(clippy::missing_const_for_fn, reason = "stands in for an OS call")]
    pub(crate) fn lock_shared(&self) -> io::Result<()> {
        Err(io::const_error!(
            io::ErrorKind::Unsupported,
            "lock_shared() not supported"
        ))
    }

    #[cfg(target_os = "wasi")]
    #[allow(clippy::unused_self, reason = "std's signature")]
    #[allow(clippy::missing_const_for_fn, reason = "stands in for an OS call")]
    pub(crate) fn try_lock(&self) -> Result<(), TryLockError> {
        Err(TryLockError::Error(io::const_error!(
            io::ErrorKind::Unsupported,
            "try_lock() not supported"
        )))
    }

    #[cfg(target_os = "wasi")]
    #[allow(clippy::unused_self, reason = "std's signature")]
    #[allow(clippy::missing_const_for_fn, reason = "stands in for an OS call")]
    pub(crate) fn try_lock_shared(&self) -> Result<(), TryLockError> {
        Err(TryLockError::Error(io::const_error!(
            io::ErrorKind::Unsupported,
            "try_lock_shared() not supported"
        )))
    }

    #[cfg(target_os = "wasi")]
    #[allow(clippy::unused_self, reason = "std's signature")]
    #[allow(clippy::missing_const_for_fn, reason = "stands in for an OS call")]
    pub(crate) fn unlock(&self) -> io::Result<()> {
        Err(io::const_error!(
            io::ErrorKind::Unsupported,
            "unlock() not supported"
        ))
    }

    pub(crate) fn truncate(&self, size: u64) -> io::Result<()> {
        let Ok(size) = i64::try_from(size) else {
            return Err(io::const_error!(
                io::ErrorKind::InvalidInput,
                "number too large to fit in target type",
            ));
        };
        // SAFETY: `ftruncate` touches no memory.
        cvt_r(|| unsafe { ftruncate64(self.fd(), size) }).map(drop)
    }

    pub(crate) fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: `read_uninit` lends the buffer to the kernel alone.
        self.read_uninit(unsafe { io::as_uninit(buf) })
    }

    /// Reads into possibly uninitialized memory. The kernel initializes the
    /// bytes it reports as read, and writes nothing else.
    pub(crate) fn read_uninit(
        &self,
        buf: &mut [MaybeUninit<u8>],
    ) -> io::Result<usize> {
        os::read(self.fd(), buf)
    }

    pub(crate) fn read_vectored(
        &self,
        bufs: &mut [IoSliceMut<'_>],
    ) -> io::Result<usize> {
        os::read_vectored(self.fd(), bufs)
    }

    #[cfg(unix)]
    pub(crate) fn read_at(
        &self,
        buf: &mut [u8],
        offset: u64,
    ) -> io::Result<usize> {
        let len = buf.len().min(os::MAX_LEN);
        let offset = file_offset(offset);
        // SAFETY: `buf` is valid for writes of `len` bytes.
        os::cvt_len(unsafe {
            pread64(self.fd(), buf.as_mut_ptr().cast(), len, offset)
        })
    }

    pub(crate) fn write(&self, buf: &[u8]) -> io::Result<usize> {
        os::write(self.fd(), buf)
    }

    pub(crate) fn write_vectored(
        &self,
        bufs: &[IoSlice<'_>],
    ) -> io::Result<usize> {
        os::write_vectored(self.fd(), bufs)
    }

    #[cfg(unix)]
    pub(crate) fn write_at(
        &self,
        buf: &[u8],
        offset: u64,
    ) -> io::Result<usize> {
        let len = buf.len().min(os::MAX_LEN);
        let offset = file_offset(offset);
        // SAFETY: `buf` is valid for reads of `len` bytes.
        os::cvt_len(unsafe {
            pwrite64(self.fd(), buf.as_ptr().cast(), len, offset)
        })
    }

    #[allow(clippy::unused_self, clippy::unnecessary_wraps)]
    pub(crate) const fn flush(&self) -> io::Result<()> {
        Ok(())
    }

    pub(crate) fn seek(&self, pos: SeekFrom) -> io::Result<u64> {
        let (whence, offset) = match pos {
            SeekFrom::Start(offset) => (libc::SEEK_SET, file_offset(offset)),
            SeekFrom::End(offset) => (libc::SEEK_END, offset),
            SeekFrom::Current(offset) => (libc::SEEK_CUR, offset),
        };
        // SAFETY: `lseek` touches no memory.
        cvt(unsafe { lseek64(self.fd(), offset, whence) })
            .map(i64::unsigned_abs)
    }

    pub(crate) fn tell(&self) -> io::Result<u64> {
        self.seek(SeekFrom::Current(0))
    }

    pub(crate) fn duplicate(&self) -> io::Result<Self> {
        self.0.try_clone().map(Self)
    }

    pub(crate) fn set_permissions(
        &self,
        perm: FilePermissions,
    ) -> io::Result<()> {
        // SAFETY: `fchmod` touches no memory.
        cvt_r(|| unsafe { libc::fchmod(self.fd(), perm.mode) }).map(drop)
    }

    #[cfg(not(target_vendor = "apple"))]
    pub(crate) fn set_times(&self, times: FileTimes) -> io::Result<()> {
        let times = times.to_timespecs()?;
        // SAFETY: `times` holds the two timestamps `futimens` reads.
        cvt(unsafe { libc::futimens(self.fd(), times.as_ptr()) }).map(drop)
    }

    #[cfg(target_vendor = "apple")]
    pub(crate) fn set_times(&self, times: FileTimes) -> io::Result<()> {
        times::set(times::Target::Fd(self.fd()), times)
    }
}

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let fd = self.fd();
        let mut b = f.debug_struct("File");
        b.field("fd", &fd);
        if let Some(path) = fd_path(fd) {
            b.field("path", &path);
        }
        if let Some((read, write)) = access_mode(fd) {
            b.field("read", &read).field("write", &write);
        }
        b.finish()
    }
}

/// Returns the path of the file `fd` refers to, as `/proc` reports it.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn fd_path(fd: RawFd) -> Option<PathBuf> {
    // Only `Debug` asks, so the formatting and the allocation do not matter.
    readlink(Path::new(&alloc_crate::format!("/proc/self/fd/{fd}"))).ok()
}

/// Returns the path of the file `fd` refers to, as `F_GETPATH` reports it,
/// or on NetBSD, where that fails, `/proc` as std falls back to.
#[cfg(any(target_vendor = "apple", target_os = "netbsd"))]
fn fd_path(fd: RawFd) -> Option<PathBuf> {
    // `F_GETPATH` writes up to `MAXPATHLEN` bytes, NUL included.
    let mut buf =
        alloc_crate::vec![0u8; libc::PATH_MAX.unsigned_abs() as usize];
    // SAFETY: `buf` is valid for writes of `MAXPATHLEN` bytes.
    if unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) } == -1 {
        #[cfg(target_os = "netbsd")]
        return readlink(Path::new(&alloc_crate::format!(
            "/proc/self/fd/{fd}"
        )))
        .ok();
        #[cfg(not(target_os = "netbsd"))]
        return None;
    }
    let len = CStr::from_bytes_until_nul(&buf).ok()?.count_bytes();
    buf.truncate(len);
    Some(PathBuf::from(OsString::from_unix_vec(buf)))
}

/// Returns the path of the file `fd` refers to, as `F_KINFO` reports it.
#[cfg(target_os = "freebsd")]
fn fd_path(fd: RawFd) -> Option<PathBuf> {
    // SAFETY: `kinfo_file` holds only integers and byte arrays, for which
    // zero is valid.
    let mut info: libc::kinfo_file = unsafe { core::mem::zeroed() };
    // The kernel checks the size before it fills in the rest.
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let size = size_of::<libc::kinfo_file>() as libc::c_int;
    info.kf_structsize = size;
    // SAFETY: `info` is a writable `kinfo_file` of the size it declares.
    if unsafe { libc::fcntl(fd, libc::F_KINFO, &raw mut info) } == -1 {
        return None;
    }
    // The path is `c_char`s, as bytes.
    #[allow(clippy::cast_sign_loss)]
    let path = info.kf_path.map(|byte| byte as u8);
    let path = CStr::from_bytes_until_nul(&path).ok()?.to_bytes();
    Some(PathBuf::from(OsString::from_unix_vec(path.to_vec())))
}

/// OpenBSD, DragonFly and WASI offer no path for a descriptor, as in std.
#[cfg(any(target_os = "openbsd", target_os = "dragonfly", target_os = "wasi"))]
const fn fd_path(_: RawFd) -> Option<PathBuf> {
    None
}

/// Returns whether `fd` is open for reading and for writing.
fn access_mode(fd: RawFd) -> Option<(bool, bool)> {
    // SAFETY: `F_GETFL` touches no memory.
    let mode = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if mode == -1 {
        return None;
    }
    match mode & libc::O_ACCMODE {
        libc::O_RDONLY => Some((true, false)),
        libc::O_RDWR => Some((true, true)),
        libc::O_WRONLY => Some((false, true)),
        _ => None,
    }
}

/// Options for opening a file, with std's defaults and validation.
#[derive(Clone)]
#[allow(clippy::struct_excessive_bools, reason = "std's options, one each")]
pub(crate) struct OpenOptions {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
    custom_flags: i32,
    mode: libc::mode_t,
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
            mode: 0o666,
        }
    }

    pub(crate) const fn read(&mut self, read: bool) {
        self.read = read;
    }

    pub(crate) const fn write(&mut self, write: bool) {
        self.write = write;
    }

    pub(crate) const fn append(&mut self, append: bool) {
        self.append = append;
    }

    pub(crate) const fn truncate(&mut self, truncate: bool) {
        self.truncate = truncate;
    }

    pub(crate) const fn create(&mut self, create: bool) {
        self.create = create;
    }

    pub(crate) const fn create_new(&mut self, create_new: bool) {
        self.create_new = create_new;
    }

    #[cfg(unix)]
    pub(crate) const fn custom_flags(&mut self, flags: i32) {
        self.custom_flags = flags;
    }

    #[cfg(unix)]
    pub(crate) const fn mode(&mut self, mode: u32) {
        self.mode = to_mode(mode);
    }

    const fn access_mode(&self) -> io::Result<c_int> {
        match (self.read, self.write, self.append) {
            (true, false, false) => Ok(libc::O_RDONLY),
            (false, true, false) => Ok(libc::O_WRONLY),
            (true, true, false) => Ok(libc::O_RDWR),
            (false, _, true) => Ok(libc::O_WRONLY | libc::O_APPEND),
            (true, _, true) => Ok(libc::O_RDWR | libc::O_APPEND),
            (false, false, false) => {
                Err(if self.create || self.create_new || self.truncate {
                    needs_write_error()
                } else {
                    io::const_error!(
                        io::ErrorKind::InvalidInput,
                        "must specify at least one of read, write, or append \
                         access",
                    )
                })
            }
        }
    }

    const fn creation_mode(&self) -> io::Result<c_int> {
        match (self.write, self.append) {
            (true, false) => {}
            (false, false) => {
                if self.truncate || self.create || self.create_new {
                    return Err(needs_write_error());
                }
            }
            (_, true) => {
                if self.truncate && !self.create_new {
                    return Err(io::const_error!(
                        io::ErrorKind::InvalidInput,
                        "append and truncate cannot both be enabled",
                    ));
                }
            }
        }
        Ok(match (self.create, self.truncate, self.create_new) {
            (false, false, false) => 0,
            (true, false, false) => libc::O_CREAT,
            (false, true, false) => libc::O_TRUNC,
            (true, true, false) => libc::O_CREAT | libc::O_TRUNC,
            (_, _, true) => libc::O_CREAT | libc::O_EXCL,
        })
    }
}

const fn needs_write_error() -> io::Error {
    io::const_error!(
        io::ErrorKind::InvalidInput,
        "creating or truncating a file requires write or append access",
    )
}

impl fmt::Debug for OpenOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenOptions")
            .field("read", &self.read)
            .field("write", &self.write)
            .field("append", &self.append)
            .field("truncate", &self.truncate)
            .field("create", &self.create)
            .field("create_new", &self.create_new)
            .field("custom_flags", &self.custom_flags)
            .field("mode", &Mode(self.mode))
            .finish()
    }
}

/// The permission bits of a file, with its type bits: the whole `st_mode`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct FilePermissions {
    mode: libc::mode_t,
}

impl FilePermissions {
    pub(crate) const fn from_mode(mode: u32) -> Self {
        Self {
            mode: to_mode(mode),
        }
    }

    pub(crate) const fn mode(self) -> u32 {
        mode_u32(self.mode)
    }

    /// Whether no class of users may write.
    pub(crate) const fn readonly(self) -> bool {
        self.mode & 0o222 == 0
    }

    /// Removes or adds write permission for every class, like `chmod a-w`
    /// or `chmod a+w`.
    pub(crate) const fn set_readonly(&mut self, readonly: bool) {
        if readonly {
            self.mode &= !0o222;
        } else {
            self.mode |= 0o222;
        }
    }
}

impl fmt::Debug for FilePermissions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FilePermissions")
            .field("mode", &Mode(self.mode))
            .finish()
    }
}

/// The type of a file: the `S_IFMT` bits of its mode.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct FileType {
    format: libc::mode_t,
}

impl FileType {
    const fn new(mode: libc::mode_t) -> Self {
        Self {
            format: mode & libc::S_IFMT,
        }
    }

    /// Whether the type is `format`, one of the `S_IF*` constants.
    pub(crate) const fn is(self, format: libc::mode_t) -> bool {
        self.format == format
    }

    pub(crate) const fn is_dir(self) -> bool {
        self.is(libc::S_IFDIR)
    }

    pub(crate) const fn is_file(self) -> bool {
        self.is(libc::S_IFREG)
    }

    pub(crate) const fn is_symlink(self) -> bool {
        self.is(libc::S_IFLNK)
    }
}

/// The times to set on a file; `None` leaves a time unchanged.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FileTimes {
    accessed: Option<SystemTime>,
    modified: Option<SystemTime>,
    /// The birth time, which macOS lets a program set.
    #[cfg(target_vendor = "apple")]
    created: Option<SystemTime>,
}

impl FileTimes {
    pub(crate) const fn set_accessed(&mut self, t: SystemTime) {
        self.accessed = Some(t);
    }

    pub(crate) const fn set_modified(&mut self, t: SystemTime) {
        self.modified = Some(t);
    }

    #[cfg(target_vendor = "apple")]
    pub(crate) const fn set_created(&mut self, t: SystemTime) {
        self.created = Some(t);
    }

    /// The access and modification times, as `futimens` and `utimensat` take.
    #[cfg(not(target_vendor = "apple"))]
    fn to_timespecs(self) -> io::Result<[libc::timespec; 2]> {
        Ok([timespec(self.accessed)?, timespec(self.modified)?])
    }
}

/// Converts a time for the C library, or `None` to `UTIME_OMIT`.
fn timespec(time: Option<SystemTime>) -> io::Result<libc::timespec> {
    let Some(time) = time else {
        return Ok(libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_OMIT,
        });
    };
    let (secs, nanos) = time.to_unix();
    // `time_t` has 32 bits on some 32-bit targets, and libc deprecates the
    // alias on musl ahead of its 64-bit switch.
    #[allow(clippy::useless_conversion, deprecated)]
    let Some(tv_sec) = libc::time_t::try_from(secs).ok() else {
        return Err(if secs > 0 {
            io::const_error!(
                io::ErrorKind::InvalidInput,
                "timestamp is too large to set as a file time",
            )
        } else {
            io::const_error!(
                io::ErrorKind::InvalidInput,
                "timestamp is too small to set as a file time",
            )
        });
    };
    // The nanoseconds are below one billion, which fits every `c_long`.
    #[allow(clippy::unnecessary_fallible_conversions)]
    Ok(libc::timespec {
        tv_sec,
        tv_nsec: libc::c_long::try_from(nanos).unwrap_or(0),
    })
}

/// Creates directories with a mode, `0o777` minus the umask by default.
pub(crate) struct DirBuilder {
    mode: libc::mode_t,
}

impl DirBuilder {
    pub(crate) const fn new() -> Self {
        Self { mode: 0o777 }
    }

    #[cfg(unix)]
    pub(crate) const fn set_mode(&mut self, mode: u32) {
        self.mode = to_mode(mode);
    }

    pub(crate) fn mkdir(&self, path: &Path) -> io::Result<()> {
        let mut buf = STACK_BUF;
        let path = cstr(path, &mut buf)?;
        // SAFETY: `path` is a valid C string.
        cvt(unsafe { libc::mkdir(path.as_ptr(), self.mode) }).map(drop)
    }
}

impl fmt::Debug for DirBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DirBuilder")
            .field("mode", &Mode(self.mode))
            .finish()
    }
}

/// Formats a mode in octal, followed by its `ls -l` form for the file
/// types `ls` knows, as std does: `0o100644 (-rw-r--r--)`.
struct Mode(libc::mode_t);

impl fmt::Debug for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        type ModeT = libc::mode_t;
        let mode = self.0;
        write!(f, "0o{mode:06o}")?;
        let kind = match mode & libc::S_IFMT {
            libc::S_IFDIR => b'd',
            libc::S_IFBLK => b'b',
            libc::S_IFCHR => b'c',
            libc::S_IFLNK => b'l',
            libc::S_IFIFO => b'p',
            libc::S_IFREG => b'-',
            _ => return Ok(()),
        };
        let bit = |bit: ModeT, letter: u8| {
            if mode & bit == 0 { b'-' } else { letter }
        };
        // The setuid, setgid and, for directories, sticky bits replace the
        // execute letters: lowercase when the execute bit is set too.
        let exec = |exec: ModeT, special: ModeT, lower: u8, upper: u8| match (
            mode & exec != 0,
            mode & special != 0,
        ) {
            (true, true) => lower,
            (false, true) => upper,
            (true, false) => b'x',
            (false, false) => b'-',
        };
        let sticky = if kind == b'd' { libc::S_ISVTX } else { 0 };
        let text = [
            b' ',
            b'(',
            kind,
            bit(libc::S_IRUSR, b'r'),
            bit(libc::S_IWUSR, b'w'),
            exec(libc::S_IXUSR, libc::S_ISUID, b's', b'S'),
            bit(libc::S_IRGRP, b'r'),
            bit(libc::S_IWGRP, b'w'),
            exec(libc::S_IXGRP, libc::S_ISGID, b's', b'S'),
            bit(libc::S_IROTH, b'r'),
            bit(libc::S_IWOTH, b'w'),
            exec(libc::S_IXOTH, sticky, b't', b'T'),
            b')',
        ];
        // `text` is ASCII.
        core::str::from_utf8(&text).map_or(Ok(()), |text| f.write_str(text))
    }
}

pub(crate) fn unlink(path: &Path) -> io::Result<()> {
    call_path(path, libc::unlink)
}

pub(crate) fn rmdir(path: &Path) -> io::Result<()> {
    call_path(path, libc::rmdir)
}

pub(crate) fn rename(from: &Path, to: &Path) -> io::Result<()> {
    call_paths(from, to, libc::rename)
}

#[cfg(unix)]
pub(crate) fn symlink(original: &Path, link: &Path) -> io::Result<()> {
    call_paths(original, link, libc::symlink)
}

pub(crate) fn link(original: &Path, link: &Path) -> io::Result<()> {
    let (mut buf_a, mut buf_b) = (STACK_BUF, STACK_BUF);
    let original = cstr(original, &mut buf_a)?;
    let link = cstr(link, &mut buf_b)?;
    // Unlike `link`, whose behavior for a symlink `original` POSIX leaves
    // open, `linkat` without `AT_SYMLINK_FOLLOW` links the symlink itself.
    // SAFETY: both paths are valid C strings.
    cvt(unsafe {
        libc::linkat(
            libc::AT_FDCWD,
            original.as_ptr(),
            libc::AT_FDCWD,
            link.as_ptr(),
            0,
        )
    })
    .map(drop)
}

pub(crate) fn stat(path: &Path) -> io::Result<FileAttr> {
    let mut buf = STACK_BUF;
    attr::stat(&cstr(path, &mut buf)?, true)
}

pub(crate) fn lstat(path: &Path) -> io::Result<FileAttr> {
    let mut buf = STACK_BUF;
    attr::stat(&cstr(path, &mut buf)?, false)
}

pub(crate) fn exists(path: &Path) -> io::Result<bool> {
    match stat(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

pub(crate) fn readlink(path: &Path) -> io::Result<PathBuf> {
    let mut buf = STACK_BUF;
    sys_path::readlink(&cstr(path, &mut buf)?)
}

pub(crate) fn canonicalize(path: &Path) -> io::Result<PathBuf> {
    let mut buf = STACK_BUF;
    sys_path::realpath(&cstr(path, &mut buf)?)
}

pub(crate) fn set_perm(path: &Path, perm: FilePermissions) -> io::Result<()> {
    let mut buf = STACK_BUF;
    let path = cstr(path, &mut buf)?;
    // SAFETY: `path` is a valid C string.
    cvt_r(|| unsafe { libc::chmod(path.as_ptr(), perm.mode) }).map(drop)
}

pub(crate) fn set_times(path: &Path, times: FileTimes) -> io::Result<()> {
    set_times_at(path, times, true)
}

pub(crate) fn set_times_nofollow(
    path: &Path,
    times: FileTimes,
) -> io::Result<()> {
    set_times_at(path, times, false)
}

#[cfg(not(target_vendor = "apple"))]
fn set_times_at(path: &Path, times: FileTimes, follow: bool) -> io::Result<()> {
    let mut buf = STACK_BUF;
    let path = cstr(path, &mut buf)?;
    let times = times.to_timespecs()?;
    let flags = if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
    // SAFETY: `path` is a valid C string and `times` holds the two
    // timestamps `utimensat` reads.
    cvt(unsafe {
        libc::utimensat(libc::AT_FDCWD, path.as_ptr(), times.as_ptr(), flags)
    })
    .map(drop)
}

#[cfg(target_vendor = "apple")]
fn set_times_at(path: &Path, times: FileTimes, follow: bool) -> io::Result<()> {
    let mut buf = STACK_BUF;
    let path = cstr(path, &mut buf)?;
    times::set(
        times::Target::Path {
            path: &path,
            follow,
        },
        times,
    )
}

#[cfg(unix)]
pub(crate) fn chown(path: &Path, uid: u32, gid: u32) -> io::Result<()> {
    let mut buf = STACK_BUF;
    let path = cstr(path, &mut buf)?;
    // SAFETY: `path` is a valid C string.
    cvt(unsafe { libc::chown(path.as_ptr(), uid, gid) }).map(drop)
}

#[cfg(unix)]
pub(crate) fn lchown(path: &Path, uid: u32, gid: u32) -> io::Result<()> {
    let mut buf = STACK_BUF;
    let path = cstr(path, &mut buf)?;
    // SAFETY: `path` is a valid C string.
    cvt(unsafe { libc::lchown(path.as_ptr(), uid, gid) }).map(drop)
}

#[cfg(unix)]
pub(crate) fn fchown(fd: RawFd, uid: u32, gid: u32) -> io::Result<()> {
    // SAFETY: `fchown` touches no memory.
    cvt(unsafe { libc::fchown(fd, uid, gid) }).map(drop)
}

#[cfg(unix)]
pub(crate) fn chroot(dir: &Path) -> io::Result<()> {
    call_path(dir, libc::chroot)
}

/// Makes `path`, which is not empty, absolute without touching the file
/// system except to read the working directory, as POSIX resolves paths.
pub(crate) fn absolute(path: &Path) -> io::Result<PathBuf> {
    let bytes = path.as_os_str().as_encoded_bytes();
    let mut components = path.strip_prefix(".").unwrap_or(path).components();
    let mut normalized = if path.is_absolute() {
        // POSIX gives exactly two leading slashes an implementation-defined
        // meaning, and treats more as one.
        if bytes.starts_with(b"//") && !bytes.starts_with(b"///") {
            components.next();
            PathBuf::from("//")
        } else {
            PathBuf::new()
        }
    } else {
        sys_path::getcwd()?
    };
    normalized.extend(components);
    // A trailing slash requires a directory and follows a final symlink, so
    // it stays.
    if bytes.ends_with(b"/") {
        normalized.push("");
    }
    Ok(normalized)
}
