//! The standard stream handles and their locks.

use core::{
    cell::UnsafeCell,
    fmt,
    marker::PhantomData,
    panic::{RefUnwindSafe, UnwindSafe},
};

use alloc_crate::{string::String, vec::Vec};

use super::{IsTerminal, private};
#[cfg(any(unix, target_os = "wasi"))]
use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};
#[cfg(windows)]
use crate::os::windows::io::{
    AsHandle, AsRawHandle, BorrowedHandle, RawHandle,
};
use crate::{
    io::{
        BufRead, BufReader, DEFAULT_BUF_SIZE, Error, IoSlice, IoSliceMut,
        Lines, Read, Result, Write, write::FORMATTER_ERROR,
    },
    stdio::{Output, OutputGuard, WriteError},
    sync::raw_mutex::RawMutex,
    sys::stdio as sys_stdio,
};

/// The OS stream under the buffer of [`Stdin`]. A stream without a valid
/// descriptor or handle reads nothing, as in std.
struct StdinRaw(sys_stdio::Stdin);

impl Read for StdinRaw {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        match self.0.read(buf) {
            Err(e) if e.raw_os_error().is_some_and(sys_stdio::is_ebadf) => {
                Ok(0)
            }
            result => result,
        }
    }
}

/// The buffer of standard input and the lock that guards it.
struct StdinBuffer {
    mutex: RawMutex,
    /// Allocated on the first read, as std allocates it on the first use.
    reader: UnsafeCell<Option<BufReader<StdinRaw>>>,
}

// SAFETY: only the thread holding `mutex` accesses `reader`, which may move
// between threads since it is `Send`; the mutex's `Acquire` and `Release`
// order the accesses of successive holders.
unsafe impl Sync for StdinBuffer {}

static STDIN: StdinBuffer = StdinBuffer {
    mutex: RawMutex::new(),
    reader: UnsafeCell::new(None),
};

/// A handle to the standard input stream of a process.
///
/// Each handle is a shared reference to a global buffer of input data,
/// guarded by a lock; [`Stdin::lock`] gives full access to the [`BufRead`]
/// methods. Created by [`stdin`].
///
/// On Windows, console input is read as UTF-16 and converted to UTF-8, and
/// input that is not valid UTF-16 fails with an error. A process without a
/// console, such as a GUI program, has no handle, and reads nothing.
pub struct Stdin {
    _private: (),
}

/// A locked reference to the [`Stdin`] handle, which implements [`Read`]
/// and [`BufRead`], created by [`Stdin::lock`].
#[must_use = "if unused stdin will immediately unlock"]
pub struct StdinLock<'a> {
    _marker: PhantomData<(&'a (), *const ())>,
}

// SAFETY: a shared `StdinLock` reaches only `is_terminal`, the descriptor
// or handle, and `Debug`, none of which touches the buffer.
unsafe impl Sync for StdinLock<'_> {}

/// Constructs a new handle to the standard input of the current process.
///
/// Each handle refers to one shared buffer, whose 8 KiB are allocated on
/// the first read, guarded by a lock. See [`Stdin::lock`] for explicit
/// control over locking.
#[must_use]
#[allow(clippy::missing_const_for_fn, reason = "not `const` in std")]
pub fn stdin() -> Stdin {
    Stdin { _private: () }
}

impl Stdin {
    /// Locks this handle to the standard input stream, returning a readable
    /// guard, which releases the lock when dropped. Blocks the current
    /// thread while another thread holds the lock; the lock is not
    /// reentrant.
    #[allow(clippy::unused_self, reason = "std's signature")]
    pub fn lock(&self) -> StdinLock<'static> {
        STDIN.mutex.lock();
        StdinLock {
            _marker: PhantomData,
        }
    }

    /// Locks this handle and reads a line of input, appending it to `buf`;
    /// see [`BufRead::read_line`].
    ///
    /// # Errors
    ///
    /// Returns the errors of [`BufRead::read_line`].
    pub fn read_line(&self, buf: &mut String) -> Result<usize> {
        self.lock().read_line(buf)
    }

    /// Consumes this handle and returns an iterator over input lines; see
    /// [`BufRead::lines`].
    #[must_use = "`self` will be dropped if the result is not used"]
    #[allow(clippy::needless_pass_by_value, reason = "std's signature")]
    pub fn lines(self) -> Lines<StdinLock<'static>> {
        self.lock().lines()
    }
}

impl StdinLock<'_> {
    /// Returns the buffered reader, allocating it on first use.
    #[allow(
        clippy::unused_self,
        clippy::needless_pass_by_ref_mut,
        reason = "`&mut self` makes the borrow exclusive"
    )]
    fn inner(&mut self) -> &mut BufReader<StdinRaw> {
        // SAFETY: the pointer comes from a static's `UnsafeCell`, so it is
        // valid and aligned. This lock holds `STDIN.mutex`, so no other
        // thread accesses the reader, and `&mut self` keeps this the only
        // borrow: the mutex is not reentrant, so no second `StdinLock` exists.
        let reader = unsafe { &mut *STDIN.reader.get() };
        reader.get_or_insert_with(|| {
            let raw = StdinRaw(sys_stdio::Stdin::new());
            BufReader::with_capacity(DEFAULT_BUF_SIZE, raw)
        })
    }
}

impl Drop for StdinLock<'_> {
    fn drop(&mut self) {
        // SAFETY: this lock took the mutex in `Stdin::lock`, and gives up
        // the reader with it.
        unsafe { STDIN.mutex.unlock() };
    }
}

/// Implements `Read` for the types that read through a locked buffer, by
/// forwarding every method to what the method `$via` returns.
macro_rules! read_through {
    ($($ty:ty => $via:ident),+ $(,)?) => {$(
        impl Read for $ty {
            fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
                self.$via().read(buf)
            }

            fn read_vectored(
                &mut self,
                bufs: &mut [IoSliceMut<'_>],
            ) -> Result<usize> {
                self.$via().read_vectored(bufs)
            }

            fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
                self.$via().read_to_end(buf)
            }

            fn read_to_string(&mut self, buf: &mut String) -> Result<usize> {
                self.$via().read_to_string(buf)
            }

            fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
                self.$via().read_exact(buf)
            }
        }
    )+};
}

read_through! {
    Stdin => lock,
    &Stdin => lock,
    StdinLock<'_> => inner,
}

impl BufRead for StdinLock<'_> {
    fn fill_buf(&mut self) -> Result<&[u8]> {
        self.inner().fill_buf()
    }

    fn consume(&mut self, amount: usize) {
        self.inner().consume(amount);
    }

    fn read_until(&mut self, byte: u8, buf: &mut Vec<u8>) -> Result<usize> {
        self.inner().read_until(byte, buf)
    }

    fn skip_until(&mut self, byte: u8) -> Result<usize> {
        self.inner().skip_until(byte)
    }

    fn read_line(&mut self, buf: &mut String) -> Result<usize> {
        self.inner().read_line(buf)
    }
}

/// A handle to the global standard output stream of the current process.
///
/// Access is synchronized by a lock, which [`Stdout::lock`] takes for as
/// long as its guard lives; the `print!` macros take it too, so output
/// from other threads cannot come in between. Created by [`stdout`].
///
/// Unlike std's, the stream is not line-buffered: every write reaches the
/// OS before it returns, `write!` and `writeln!` in one write when the
/// text fits in 1 KiB, so [`flush`](Write::flush) has nothing to do and no
/// output is lost when the process aborts. Many small writes cost a system
/// call each; wrap the lock in a [`BufWriter`](crate::io::BufWriter) for those.
///
/// On Windows, as in std, a console gets the text as UTF-16, so it shows
/// every character whatever its code page, and bytes that are not UTF-8 fail
/// to write to it unless its code page is UTF-8. A process without a
/// console, such as a GUI program, has no handle, and discards the output.
pub struct Stdout {
    _private: (),
}

/// A locked reference to the [`Stdout`] handle, which implements [`Write`],
/// created by [`Stdout::lock`].
#[must_use = "if unused stdout will immediately unlock"]
pub struct StdoutLock<'a> {
    inner: OutputGuard,
    _lifetime: PhantomData<&'a ()>,
}

/// Constructs a new handle to the standard output of the current process.
///
/// Its writes are not buffered; see [`Stdout`].
#[must_use]
#[allow(clippy::missing_const_for_fn, reason = "not `const` in std")]
pub fn stdout() -> Stdout {
    Stdout { _private: () }
}

impl Stdout {
    /// Locks this handle to the standard output stream, returning a writable
    /// guard, which releases the lock when dropped.
    ///
    /// The lock is reentrant: a thread holding it may lock it again, and
    /// print. Blocks the current thread while another thread holds it.
    #[allow(clippy::unused_self, reason = "std's signature")]
    pub fn lock(&self) -> StdoutLock<'static> {
        StdoutLock {
            inner: Output::stdout().lock(),
            _lifetime: PhantomData,
        }
    }
}

/// A handle to the standard error stream of a process, created by
/// [`stderr`].
///
/// It works like [`Stdout`], with a lock of its own.
pub struct Stderr {
    _private: (),
}

/// A locked reference to the [`Stderr`] handle, which implements [`Write`],
/// created by [`Stderr::lock`].
#[must_use = "if unused stderr will immediately unlock"]
pub struct StderrLock<'a> {
    inner: OutputGuard,
    _lifetime: PhantomData<&'a ()>,
}

/// Constructs a new handle to the standard error of the current process.
///
/// This handle is not buffered.
#[must_use]
#[allow(clippy::missing_const_for_fn, reason = "not `const` in std")]
pub fn stderr() -> Stderr {
    Stderr { _private: () }
}

impl Stderr {
    /// Locks this handle to the standard error stream, returning a writable
    /// guard, which releases the lock when dropped.
    ///
    /// The lock is reentrant: a thread holding it may lock it again, and
    /// print. Blocks the current thread while another thread holds it.
    #[allow(clippy::unused_self, reason = "std's signature")]
    pub fn lock(&self) -> StderrLock<'static> {
        StderrLock {
            inner: Output::stderr().lock(),
            _lifetime: PhantomData,
        }
    }
}

/// Converts the failure of a write to a standard stream to an I/O error.
fn io_error(error: WriteError) -> Error {
    match error {
        WriteError::Os(code) => sys_stdio::write_error(code),
        WriteError::Zero => Error::WRITE_ALL_EOF,
        WriteError::Format => FORMATTER_ERROR,
    }
}

/// The result of `Write::write` on a standard stream: a stream without a
/// valid descriptor or handle takes every byte, as in std.
fn write_result(
    result: core::result::Result<usize, WriteError>,
    len: usize,
) -> Result<usize> {
    match result {
        Err(WriteError::Os(code)) if sys_stdio::is_ebadf(code) => Ok(len),
        result => result.map_err(io_error),
    }
}

/// Implements `Write` for the handles, which lock their stream for each call:
/// every method forwards to what the method `$via` returns.
macro_rules! write_through {
    ($($ty:ty => $via:ident),+ $(,)?) => {$(
        impl Write for $ty {
            fn write(&mut self, buf: &[u8]) -> Result<usize> {
                self.$via().write(buf)
            }

            fn write_vectored(
                &mut self,
                bufs: &[IoSlice<'_>],
            ) -> Result<usize> {
                self.$via().write_vectored(bufs)
            }

            fn flush(&mut self) -> Result<()> {
                self.$via().flush()
            }

            fn write_all(&mut self, buf: &[u8]) -> Result<()> {
                self.$via().write_all(buf)
            }

            fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> Result<()> {
                self.$via().write_fmt(args)
            }
        }
    )+};
}

write_through! {
    Stdout => lock,
    &Stdout => lock,
    Stderr => lock,
    &Stderr => lock,
}

/// Implements `Debug` for a handle and its lock, which show their names only.
macro_rules! debug_impls {
    ($handle:ident, $lock:ident) => {
        impl fmt::Debug for $handle {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($handle)).finish_non_exhaustive()
            }
        }

        impl fmt::Debug for $lock<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($lock)).finish_non_exhaustive()
            }
        }
    };
}

debug_impls!(Stdin, StdinLock);
debug_impls!(Stdout, StdoutLock);
debug_impls!(Stderr, StderrLock);

/// Implements `Write` for the lock of [`Stdout`] and [`Stderr`].
macro_rules! output_impls {
    ($lock:ident) => {
        impl Write for $lock<'_> {
            fn write(&mut self, buf: &[u8]) -> Result<usize> {
                write_result(self.inner.write(buf), buf.len())
            }

            fn flush(&mut self) -> Result<()> {
                self.inner.flush().map_err(io_error)
            }

            /// Formats into the stream's buffer and writes it out: one write
            /// when the text fits in 1 KiB.
            fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> Result<()> {
                self.inner.write_fmt(args, false).map_err(io_error)
            }
        }

        // As in std: the streams hold no state that an unwind could break.
        impl UnwindSafe for $lock<'_> {}
        impl RefUnwindSafe for $lock<'_> {}
    };
}

output_impls!(StdoutLock);
output_impls!(StderrLock);

/// Implements the traits that depend only on which stream a type refers
/// to: [`IsTerminal`], and the descriptor or handle traits of `os`.
macro_rules! stream_impls {
    ($($ty:ty => $stream:expr),* $(,)?) => {$(
        impl private::Sealed for $ty {}

        impl IsTerminal for $ty {
            #[inline]
            fn is_terminal(&self) -> bool {
                sys_stdio::is_terminal($stream)
            }
        }

        #[cfg(any(unix, target_os = "wasi"))]
        impl AsRawFd for $ty {
            #[inline]
            fn as_raw_fd(&self) -> RawFd {
                $stream
            }
        }

        #[cfg(any(unix, target_os = "wasi"))]
        impl AsFd for $ty {
            #[inline]
            fn as_fd(&self) -> BorrowedFd<'_> {
                // SAFETY: the standard descriptors are 0, 1 and 2, and
                // belong to the process, which no safe code closes.
                unsafe { BorrowedFd::borrow_raw($stream) }
            }
        }

        #[cfg(windows)]
        impl AsRawHandle for $ty {
            #[inline]
            fn as_raw_handle(&self) -> RawHandle {
                sys_stdio::raw_handle($stream)
            }
        }

        #[cfg(windows)]
        impl AsHandle for $ty {
            #[inline]
            fn as_handle(&self) -> BorrowedHandle<'_> {
                let handle = sys_stdio::raw_handle($stream);
                // SAFETY: the standard handles belong to the process, which
                // no safe code closes; null stands for no handle.
                unsafe { BorrowedHandle::borrow_raw(handle) }
            }
        }
    )*};
}

stream_impls!(
    Stdin => sys_stdio::STDIN,
    StdinLock<'_> => sys_stdio::STDIN,
    Stdout => sys_stdio::STDOUT,
    StdoutLock<'_> => sys_stdio::STDOUT,
    Stderr => sys_stdio::STDERR,
    StderrLock<'_> => sys_stdio::STDERR,
);
