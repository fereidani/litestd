//! [`File`], [`OpenOptions`] and [`TryLockError`].

use core::{error, fmt};

use alloc_crate::{string::String, sync::Arc, vec::Vec};

use super::{FileTimes, Metadata, Permissions};
use crate::{
    io::{self, IoSlice, IoSliceMut, Read, Seek, SeekFrom, Write},
    path::Path,
    sys,
    time::SystemTime,
};

/// An object providing access to an open file on the filesystem.
///
/// The file is closed on drop, ignoring errors; call [`sync_all`] to see
/// them. `File` does not buffer, so wrap it in a [`BufReader`] or
/// [`BufWriter`] for many small reads or writes.
///
/// # Examples
///
/// ```no_run
/// use litestd::{fs::File, io::prelude::*};
///
/// let mut file = File::create("foo.txt")?;
/// file.write_all(b"Hello, world!")?;
/// # Ok::<(), litestd::io::Error>(())
/// ```
///
/// [`sync_all`]: File::sync_all
/// [`BufReader`]: io::BufReader
/// [`BufWriter`]: io::BufWriter
pub struct File {
    pub(crate) inner: sys::fs::File,
}

/// The error of [`File::try_lock`] and [`File::try_lock_shared`].
pub enum TryLockError {
    /// The lock could not be acquired due to an I/O error on the file, which
    /// is never [`io::ErrorKind::WouldBlock`].
    Error(io::Error),
    /// The lock is held by another handle or process.
    WouldBlock,
}

impl error::Error for TryLockError {}

impl fmt::Debug for TryLockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error(err) => fmt::Debug::fmt(err, f),
            Self::WouldBlock => fmt::Debug::fmt("WouldBlock", f),
        }
    }
}

impl fmt::Display for TryLockError {
    /// Like std, honors the width, precision and alignment of `f`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(match self {
            Self::Error(_) => "lock acquisition failed due to I/O error",
            Self::WouldBlock => {
                "lock acquisition failed because the operation would block"
            }
        })
    }
}

impl From<TryLockError> for io::Error {
    fn from(err: TryLockError) -> Self {
        match err {
            TryLockError::Error(err) => err,
            TryLockError::WouldBlock => io::ErrorKind::WouldBlock.into(),
        }
    }
}

impl File {
    /// Attempts to open a file in read-only mode.
    ///
    /// # Errors
    ///
    /// Fails if `path` does not exist, and as for [`OpenOptions::open`].
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        OpenOptions::new().read(true).open(path.as_ref())
    }

    /// Opens a file in write-only mode, creating it if it does not exist and
    /// truncating it if it does.
    ///
    /// # Errors
    ///
    /// See [`OpenOptions::open`].
    pub fn create<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path.as_ref())
    }

    /// Creates a new file in read-write mode, failing if it exists. The check
    /// and the creation are one atomic operation.
    ///
    /// # Errors
    ///
    /// Fails with [`io::ErrorKind::AlreadyExists`] if the file exists, and as
    /// for [`OpenOptions::open`].
    pub fn create_new<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path.as_ref())
    }

    /// Returns a new [`OpenOptions`] object, like [`OpenOptions::new`].
    #[must_use]
    pub fn options() -> OpenOptions {
        OpenOptions::new()
    }

    /// Attempts to sync all OS-internal file content and metadata to disk.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports, such as a failed write-back.
    pub fn sync_all(&self) -> io::Result<()> {
        self.inner.fsync()
    }

    /// Like [`sync_all`](File::sync_all), but might not sync file metadata.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports, such as a failed write-back.
    pub fn sync_data(&self) -> io::Result<()> {
        self.inner.datasync()
    }

    /// Acquires an exclusive advisory lock on the file, blocking until it is
    /// available.
    ///
    /// The lock is released by [`unlock`](File::unlock) or when the file and
    /// every handle cloned from it are closed. Locking a handle that already
    /// holds a lock, directly or through a clone, may deadlock.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports; on Unix, a signal that interrupts
    /// the wait makes it fail with [`io::ErrorKind::Interrupted`].
    pub fn lock(&self) -> io::Result<()> {
        self.inner.lock()
    }

    /// Acquires a shared advisory lock on the file, blocking while an
    /// exclusive lock is held. It is released like the one of [`lock`].
    ///
    /// # Errors
    ///
    /// As for [`lock`].
    ///
    /// [`lock`]: File::lock
    pub fn lock_shared(&self) -> io::Result<()> {
        self.inner.lock_shared()
    }

    /// Tries to acquire an exclusive lock on the file without blocking.
    ///
    /// # Errors
    ///
    /// [`TryLockError::WouldBlock`] if another lock is held, and
    /// [`TryLockError::Error`] for the errors of [`lock`](File::lock).
    pub fn try_lock(&self) -> Result<(), TryLockError> {
        self.inner.try_lock()
    }

    /// Tries to acquire a shared lock on the file without blocking.
    ///
    /// # Errors
    ///
    /// [`TryLockError::WouldBlock`] if an exclusive lock is held, otherwise
    /// as for [`try_lock`](File::try_lock).
    pub fn try_lock_shared(&self) -> Result<(), TryLockError> {
        self.inner.try_lock_shared()
    }

    /// Releases all locks on the file without closing it.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn unlock(&self) -> io::Result<()> {
        self.inner.unlock()
    }

    /// Truncates or extends the file to `size` bytes, filling an extension
    /// with zeros. The cursor is not changed.
    ///
    /// # Errors
    ///
    /// Fails if the file is not open for writing, and with
    /// [`io::ErrorKind::InvalidInput`] if `size` is out of the OS's range.
    pub fn set_len(&self, size: u64) -> io::Result<()> {
        self.inner.truncate(size)
    }

    /// Queries metadata about the underlying file.
    ///
    /// # Errors
    ///
    /// Returns the error the OS reports.
    pub fn metadata(&self) -> io::Result<Metadata> {
        self.inner.file_attr().map(Metadata)
    }

    /// Creates a new `File` that shares the underlying file handle, and so
    /// its cursor, with `self`.
    ///
    /// # Errors
    ///
    /// Fails if the process has no file descriptors left.
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            inner: self.inner.duplicate()?,
        })
    }

    /// Changes the permissions on the underlying file.
    ///
    /// # Errors
    ///
    /// Fails if the user lacks permission to change the file's attributes.
    #[allow(clippy::needless_pass_by_value, reason = "std's signature")]
    pub fn set_permissions(&self, perm: Permissions) -> io::Result<()> {
        self.inner.set_permissions(perm.0)
    }

    /// Changes the timestamps of the underlying file; times left unset in
    /// `times` are not changed.
    ///
    /// # Errors
    ///
    /// Fails if the user lacks permission, and with
    /// [`io::ErrorKind::InvalidInput`] for a time the OS cannot represent.
    pub fn set_times(&self, times: FileTimes) -> io::Result<()> {
        self.inner.set_times(times.0)
    }

    /// Changes the modification time of the underlying file.
    ///
    /// # Errors
    ///
    /// As for [`set_times`](File::set_times).
    #[inline]
    pub fn set_modified(&self, time: SystemTime) -> io::Result<()> {
        self.set_times(FileTimes::new().set_modified(time))
    }
}

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

/// Returns the bytes between the cursor of `file` and its end, if known.
fn bytes_left(file: &File) -> Option<usize> {
    let size = file.inner.file_attr().ok()?.size();
    let position = file.inner.tell().ok()?;
    usize::try_from(size.saturating_sub(position)).ok()
}

impl Read for &File {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }

    #[inline]
    fn read_vectored(
        &mut self,
        bufs: &mut [IoSliceMut<'_>],
    ) -> io::Result<usize> {
        self.inner.read_vectored(bufs)
    }

    /// Reserves the bytes left in the file, then reads into spare capacity.
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        let hint = bytes_left(self);
        let file = &self.inner;
        // SAFETY: `read_uninit` lends the buffer to the OS alone, which
        // initializes the bytes it reports read and writes nothing else.
        unsafe {
            io::read_to_end_uninit(buf, hint, |spare| file.read_uninit(spare))
        }
    }

    /// Reserves the bytes left in the file, then reads into spare capacity.
    fn read_to_string(&mut self, buf: &mut String) -> io::Result<usize> {
        let hint = bytes_left(self);
        let file = &self.inner;
        // SAFETY: as in `read_to_end`.
        unsafe {
            io::read_to_string_uninit(buf, hint, |spare| {
                file.read_uninit(spare)
            })
        }
    }
}

impl Write for &File {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.inner.write_vectored(bufs)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Seek for &File {
    #[inline]
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }

    #[inline]
    fn stream_position(&mut self) -> io::Result<u64> {
        self.inner.tell()
    }
}

/// A `File`, or a pointer to one, that reads, writes and seeks through the
/// shared `&File`, as std's `Arc<File>` does.
trait ThroughRef {
    /// The file that reads, writes and seeks go to.
    fn file(&self) -> &File;
}

// No file exists on wasm32-unknown-unknown, so no reference to one does.
#[cfg_attr(target_os = "unknown", allow(clippy::uninhabited_references))]
impl ThroughRef for File {
    #[inline]
    fn file(&self) -> &Self {
        self
    }
}

#[cfg_attr(target_os = "unknown", allow(clippy::uninhabited_references))]
impl ThroughRef for Arc<File> {
    #[inline]
    fn file(&self) -> &File {
        self
    }
}

/// Implements the I/O traits for the `ThroughRef` types, forwarding every
/// method to the ones of `&File`.
macro_rules! forward_to_ref {
    ($($ty:ty),+) => {$(
        impl Read for $ty {
            #[inline]
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                self.file().read(buf)
            }

            #[inline]
            fn read_vectored(
                &mut self,
                bufs: &mut [IoSliceMut<'_>],
            ) -> io::Result<usize> {
                self.file().read_vectored(bufs)
            }

            #[inline]
            fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
                self.file().read_to_end(buf)
            }

            #[inline]
            fn read_to_string(
                &mut self,
                buf: &mut String,
            ) -> io::Result<usize> {
                self.file().read_to_string(buf)
            }

            #[inline]
            fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
                self.file().read_exact(buf)
            }
        }

        impl Write for $ty {
            #[inline]
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.file().write(buf)
            }

            #[inline]
            fn write_vectored(
                &mut self,
                bufs: &[IoSlice<'_>],
            ) -> io::Result<usize> {
                self.file().write_vectored(bufs)
            }

            #[inline]
            fn flush(&mut self) -> io::Result<()> {
                self.file().flush()
            }

            #[inline]
            fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
                self.file().write_all(buf)
            }

            #[inline]
            fn write_fmt(
                &mut self,
                args: fmt::Arguments<'_>,
            ) -> io::Result<()> {
                self.file().write_fmt(args)
            }
        }

        impl Seek for $ty {
            #[inline]
            fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
                self.file().seek(pos)
            }

            #[inline]
            fn rewind(&mut self) -> io::Result<()> {
                self.file().rewind()
            }

            #[inline]
            fn stream_position(&mut self) -> io::Result<u64> {
                self.file().stream_position()
            }

            #[inline]
            fn seek_relative(&mut self, offset: i64) -> io::Result<()> {
                self.file().seek_relative(offset)
            }
        }
    )+};
}

forward_to_ref!(File, Arc<File>);

/// Options and flags which can be used to configure how a file is opened.
#[derive(Clone, Debug)]
pub struct OpenOptions(pub(crate) sys::fs::OpenOptions);

#[allow(
    clippy::new_without_default,
    reason = "std has no `Default` for `OpenOptions`"
)]
impl OpenOptions {
    /// Creates a blank new set of options, all initially `false`.
    #[must_use]
    pub fn new() -> Self {
        Self(sys::fs::OpenOptions::new())
    }

    /// Sets the option for read access.
    pub fn read(&mut self, read: bool) -> &mut Self {
        self.0.read(read);
        self
    }

    /// Sets the option for write access. Writes to an existing file overwrite
    /// its contents without truncating it.
    pub fn write(&mut self, write: bool) -> &mut Self {
        self.0.write(write);
        self
    }

    /// Sets the option for append mode, which implies write access. Every
    /// write goes to the current end of the file, even with other writers.
    pub fn append(&mut self, append: bool) -> &mut Self {
        self.0.append(append);
        self
    }

    /// Sets the option for truncating an existing file to length 0, which
    /// needs write access.
    pub fn truncate(&mut self, truncate: bool) -> &mut Self {
        self.0.truncate(truncate);
        self
    }

    /// Sets the option to create a new file, or open it if it already
    /// exists. It needs write or append access.
    pub fn create(&mut self, create: bool) -> &mut Self {
        self.0.create(create);
        self
    }

    /// Sets the option to create a new file, failing if anything, even a
    /// dangling symlink, exists at the path. The check and the creation are
    /// one atomic operation. It needs write or append access and overrides
    /// [`create`](OpenOptions::create) and
    /// [`truncate`](OpenOptions::truncate).
    pub fn create_new(&mut self, create_new: bool) -> &mut Self {
        self.0.create_new(create_new);
        self
    }

    /// Opens a file at `path` with the options specified by `self`.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be opened as requested, and with
    /// [`io::ErrorKind::InvalidInput`] for invalid options or a NUL in `path`.
    pub fn open<P: AsRef<Path>>(&self, path: P) -> io::Result<File> {
        self.open_path(path.as_ref())
    }

    fn open_path(&self, path: &Path) -> io::Result<File> {
        Ok(File {
            inner: sys::fs::File::open(path, &self.0)?,
        })
    }
}
