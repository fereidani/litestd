//! The standard streams, through the handles that `GetStdHandle` returns.
//!
//! As in std, a console takes text as UTF-16: writes convert UTF-8 for
//! `WriteConsoleW`, unless its output code page is UTF-8, and reads convert
//! what `ReadConsoleW` returns. Other handles take and give bytes unchanged.
//! A parent may pass handles opened
//! for overlapped I/O, which `os::handle` transfers through. Writes through
//! it are serialized, so they cannot signal each other's waits; reads are
//! not, so that waiting for input never holds up output.

#[cfg(all(feature = "stdio", feature = "io"))]
use core::mem::MaybeUninit;
use core::{
    cell::UnsafeCell,
    ptr, str,
    sync::atomic::{AtomicU32, Ordering::Relaxed},
};

#[cfg(feature = "stdio")]
use windows_sys::Win32::System::{
    Console::STD_OUTPUT_HANDLE, Threading::GetCurrentThreadId,
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_INVALID_HANDLE, ERROR_NO_UNICODE_TRANSLATION, GetLastError,
        HANDLE, INVALID_HANDLE_VALUE,
    },
    System::{
        Console::{
            GetConsoleMode, GetConsoleOutputCP, GetStdHandle, STD_ERROR_HANDLE,
            STD_HANDLE, WriteConsoleW,
        },
        Threading::{
            AcquireSRWLockExclusive, ReleaseSRWLockExclusive, SRWLOCK,
            SRWLOCK_INIT,
        },
    },
};
#[cfg(all(feature = "stdio", feature = "io"))]
use windows_sys::Win32::{
    Foundation::{ERROR_OPERATION_ABORTED, SetLastError},
    System::Console::{
        CONSOLE_READCONSOLE_CONTROL, ReadConsoleW, STD_INPUT_HANDLE,
    },
};

use super::os::{
    handle::{self, Transfer},
    os_code,
};
#[cfg(all(feature = "stdio", feature = "io"))]
use crate::io;

/// A standard stream, identified by its `GetStdHandle` id.
pub(crate) type Stream = STD_HANDLE;

/// Standard input.
#[cfg(all(feature = "stdio", feature = "io"))]
pub(crate) const STDIN: Stream = STD_INPUT_HANDLE;

/// Standard output.
#[cfg(feature = "stdio")]
pub(crate) const STDOUT: Stream = STD_OUTPUT_HANDLE;

/// Standard error.
pub(crate) const STDERR: Stream = STD_ERROR_HANDLE;

/// The error code of a stream without a handle, as in std.
const NO_HANDLE: i32 = os_code(ERROR_INVALID_HANDLE);

/// The error code of a console write of bytes that are not UTF-8, as
/// `MultiByteToWideChar` reports them; [`write_error`] turns it into std's
/// error.
const NOT_UTF8: i32 = os_code(ERROR_NO_UNICODE_TRANSLATION);

/// The UTF-16 units of one console read or write: a 2 KiB stack buffer.
/// Longer text takes several calls.
const CONSOLE_UNITS: usize = 1024;

/// The first bytes of a character that console writes to standard output
/// took but could not write yet, for the next ones to finish: up to three
/// bytes, then their count, as the little-endian bytes of the value. 0
/// holds none.
static STDOUT_CARRY: AtomicU32 = AtomicU32::new(0);

/// The same for standard error.
static STDERR_CARRY: AtomicU32 = AtomicU32::new(0);

/// Serializes the process's writes through `os::handle`, so that at most
/// one of them is in flight at a time.
static WRITE_LOCK: Lock = Lock(UnsafeCell::new(SRWLOCK_INIT));

/// An SRW lock that can live in a `static`.
struct Lock(UnsafeCell<SRWLOCK>);

// SAFETY: the lock is only ever used through the SRW lock functions, which
// are made to be called on the same lock from any number of threads.
unsafe impl Sync for Lock {}

/// Returns the handle of `stream`, or null if the process has none, such as
/// a GUI program without a console. `INVALID_HANDLE_VALUE` counts as none,
/// as in std's `as_raw_handle` and documentation: `GetStdHandle` does not
/// fail for these ids, so it means the parent passed none. std's reads and
/// writes report the stale last error instead, which may be any.
pub(crate) fn raw_handle(stream: Stream) -> HANDLE {
    // SAFETY: `GetStdHandle` has no preconditions.
    let handle = unsafe { GetStdHandle(stream) };
    if handle == INVALID_HANDLE_VALUE {
        ptr::null_mut()
    } else {
        handle
    }
}

/// Writes once from `buf` to `stream`, returning the number of bytes
/// written or the Windows error code. As in std, a console gets the text as
/// UTF-16 unless its output code page is UTF-8, and anything else the bytes
/// unchanged.
pub(crate) fn write(stream: Stream, buf: &[u8]) -> Result<usize, i32> {
    if buf.is_empty() {
        return Ok(0);
    }
    let handle = raw_handle(stream);
    if handle.is_null() {
        return Err(NO_HANDLE);
    }
    if is_console(handle) && !is_utf8_console() {
        return write_console(handle, carry(stream), buf);
    }
    // SAFETY: `WRITE_LOCK` is a valid SRW lock for the life of the process,
    // and this function releases it before returning.
    unsafe { AcquireSRWLockExclusive(WRITE_LOCK.0.get()) };
    let result = handle::transfer(handle, Transfer::Write(buf), None);
    // SAFETY: this thread acquired the lock above.
    unsafe { ReleaseSRWLockExclusive(WRITE_LOCK.0.get()) };
    result
}

/// Converts a code that [`write()`] returned to an I/O error: bytes that are
/// not UTF-8 fail on a console with std's error, anything else with the OS
/// error.
#[cfg(all(feature = "stdio", feature = "io"))]
pub(crate) fn write_error(code: i32) -> io::Error {
    if code == NOT_UTF8 {
        return io::const_error!(
            io::ErrorKind::InvalidData,
            "Windows stdio in console mode does not support writing non-UTF-8 \
             byte sequences",
        );
    }
    io::Error::from_raw_os_error(code)
}

/// Forgets the first bytes of a character that console writes to `stream`
/// took for the next write to finish. The panic handler starts its report
/// afresh this way, as std's panic output has a console state of its own.
#[cfg(feature = "panic-location")]
#[cfg_attr(
    any(test, feature = "test-with-std", feature = "custom-panic-handler"),
    allow(dead_code, reason = "litestd's panic handler is compiled out")
)]
pub(crate) fn reset(stream: Stream) {
    carry(stream).store(0, Relaxed);
}

/// The first bytes of a character that console writes to the output stream
/// `stream` took, for the next write to finish.
fn carry(stream: Stream) -> &'static AtomicU32 {
    if stream == STDERR {
        &STDERR_CARRY
    } else {
        &STDOUT_CARRY
    }
}

/// Writes the longest valid UTF-8 prefix of `buf`, up to `CONSOLE_UNITS`
/// bytes, to the console `handle` as UTF-16, or adds the first byte of `buf`
/// to the character whose first bytes `carry` holds. Returns how many bytes
/// of `buf` it took. As in std, the first bytes of a character that `buf`
/// ends in count as written, and wait in `carry` for the rest, which comes a
/// byte per write.
#[inline(never)]
fn write_console(
    handle: HANDLE,
    carry: &AtomicU32,
    buf: &[u8],
) -> Result<usize, i32> {
    // Taking the carry, rather than reading it, lets one writer alone finish
    // the character should the panic handler, which writes without the
    // stream's lock, race another. That lock orders the other writers, and
    // the carry holds nothing else to order, so `Relaxed` suffices.
    let packed = carry.swap(0, Relaxed);
    // The carried bytes, then the first of `buf`, and the carried count last.
    let mut joined = packed.to_le_bytes();
    let [.., carried] = joined;
    let at = usize::from(carried);
    if let (Some(slot), Some(&first)) = (joined.get_mut(at), buf.first()) {
        *slot = first;
    }
    let input = if carried == 0 {
        buf.get(..CONSOLE_UNITS).unwrap_or(buf)
    } else {
        joined.get(..=at).unwrap_or_default()
    };
    let text = match str::from_utf8(input) {
        Ok(text) => text,
        Err(error) => {
            let valid = input.get(..error.valid_up_to()).unwrap_or_default();
            if valid.is_empty() {
                if error.error_len().is_some() {
                    return Err(NOT_UTF8);
                }
                // The carried bytes and the first of `buf` only start a
                // character, so they are 3 at most: carry them.
                joined[3] = carried + 1;
                carry.store(u32::from_le_bytes(joined), Relaxed);
                return Ok(1);
            }
            // Checked again rather than trusted in an `unsafe` block.
            str::from_utf8(valid).unwrap_or_default()
        }
    };
    let written = write_text(handle, text)?;
    // Only a console that took nothing writes less than the carried
    // character: its bytes wait for the next write again.
    if written < at {
        carry.store(packed, Relaxed);
    }
    Ok(written.saturating_sub(at))
}

/// Writes `text`, at most `CONSOLE_UNITS` bytes, to the console `handle` as
/// UTF-16, and returns how many of its bytes the console took.
fn write_text(handle: HANDLE, text: &str) -> Result<usize, i32> {
    debug_assert!(text.len() <= CONSOLE_UNITS);
    // No character has more UTF-16 units than UTF-8 bytes, so all fit.
    let mut units = [0; CONSOLE_UNITS];
    let mut len = 0;
    for (slot, unit) in units.iter_mut().zip(text.encode_utf16()) {
        *slot = unit;
        len += 1;
    }
    let units = units.get(..len).unwrap_or_default();
    let written = write_units(handle, units)?;
    // As in std, a console that stopped between the halves of a surrogate
    // pair gets the second half now, as the caller cannot write half a
    // character again. The pair counts as written even if this fails.
    if let Some(low @ [0xDC00..=0xDFFF]) = units.get(written..=written) {
        let _ = write_units(handle, low);
    }
    // The UTF-8 bytes of the characters that start in the written units. A
    // step that depends on the unit keeps the loop small, as it cannot be
    // vectorized; it ends within `written` steps, each of 1 or 2 units.
    let mut bytes = 0;
    let mut i = 0;
    while let Some(&unit) = units.get(i).filter(|_| i < written) {
        let (step, len) = match unit {
            0..0x80 => (1, 1),
            0x80..0x800 => (1, 2),
            0xD800..0xDC00 => (2, 4),
            _ => (1, 3),
        };
        i += step;
        bytes += len;
    }
    Ok(bytes)
}

/// Writes `units` to the console `handle`, returning how many it took.
fn write_units(handle: HANDLE, units: &[u16]) -> Result<usize, i32> {
    let mut written = 0;
    // SAFETY: `units` is valid for reads of its length, which `io_len` can
    // only lower, and `written` for a write; the reserved pointer is null.
    let ok = unsafe {
        WriteConsoleW(
            handle,
            units.as_ptr(),
            handle::io_len(units.len()),
            &raw mut written,
            ptr::null(),
        )
    };
    if ok == 0 {
        // SAFETY: `GetLastError` has no preconditions.
        return Err(os_code(unsafe { GetLastError() }));
    }
    Ok((written as usize).min(units.len()))
}

/// Whether `handle` is a console, which reads and writes UTF-16.
fn is_console(handle: HANDLE) -> bool {
    let mut mode = 0;
    // SAFETY: `mode` is valid for writes; any handle may be queried.
    unsafe { GetConsoleMode(handle, &raw mut mode) != 0 }
}

/// Whether the console's output code page is UTF-8, which lets it take the
/// bytes unchanged, as std writes them there.
fn is_utf8_console() -> bool {
    /// `CP_UTF8`, whose `windows-sys` feature litestd does not otherwise
    /// need here.
    const CP_UTF8: u32 = 65001;
    // SAFETY: `GetConsoleOutputCP` has no preconditions.
    unsafe { GetConsoleOutputCP() == CP_UTF8 }
}

/// Windows has no interrupted calls.
pub(crate) const fn is_interrupted(_code: i32) -> bool {
    false
}

/// Whether the error code means that the stream has no valid handle, which
/// std treats as a stream that discards output and has no input.
pub(crate) const fn is_ebadf(code: i32) -> bool {
    code == NO_HANDLE
}

/// Returns an identity of the calling thread that no other live thread
/// shares and that is never 0.
#[cfg(feature = "stdio")]
pub(crate) fn thread_id() -> usize {
    // SAFETY: `GetCurrentThreadId` has no preconditions.
    unsafe { GetCurrentThreadId() as usize }
}

/// Reads standard input: a console as UTF-16, converted to UTF-8 as in std,
/// and anything else as bytes.
#[cfg(all(feature = "stdio", feature = "io"))]
pub(crate) struct Stdin {
    /// A high surrogate that ended the last console read, for the next.
    surrogate: u16,
    /// The UTF-8 bytes of a console character that did not fit the buffer
    /// of the last read: `pending[..pending_len]`.
    pending: [u8; 4],
    pending_len: usize,
}

#[cfg(all(feature = "stdio", feature = "io"))]
impl Stdin {
    pub(crate) const fn new() -> Self {
        Self {
            surrogate: 0,
            pending: [0; 4],
            pending_len: 0,
        }
    }

    /// Reads once into `buf`. A missing handle fails with the error that
    /// [`is_ebadf`] recognizes, and a closed pipe reads 0, as in std.
    pub(crate) fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let handle = raw_handle(STDIN);
        if handle.is_null() {
            return Err(io::Error::from_raw_os_error(NO_HANDLE));
        }
        if is_console(handle) {
            return self.read_console(handle, buf);
        }
        // SAFETY: `handle::read` lends the buffer to the kernel alone.
        handle::read(handle, unsafe { io::as_uninit(buf) })
    }

    /// Reads console input into `buf` as UTF-8. Ends at a Ctrl-Z typed at
    /// the start of a read, which reads 0.
    fn read_console(
        &mut self,
        handle: HANDLE,
        buf: &mut [u8],
    ) -> io::Result<usize> {
        if self.pending_len > 0 || buf.is_empty() {
            return Ok(self.take_pending(buf));
        }
        // At most three bytes per unit, and four per surrogate pair, whose
        // high half may come from the last read: this many units fit.
        let want = (buf.len().saturating_sub(1) / 3).clamp(1, CONSOLE_UNITS);
        let mut units = [0u16; CONSOLE_UNITS + 1];
        // A read of nothing but a high surrogate converts to nothing, and
        // the second read pairs it or fails, so two reads suffice.
        for _ in 0..2 {
            // The surrogate stays carried until a read succeeds.
            let carried = usize::from(self.surrogate != 0);
            units[0] = self.surrogate;
            let fresh = units.get_mut(carried..carried + want);
            let read = read_console_units(handle, fresh.unwrap_or_default())?;
            if read == 0 {
                return Ok(0);
            }
            self.surrogate = 0;
            let mut end = carried + read;
            let last = units.get(end - 1).copied().unwrap_or_default();
            if (0xD800..0xDC00).contains(&last) {
                self.surrogate = last;
                end -= 1;
            }
            let whole = units.get(..end).unwrap_or_default();
            let written = self.encode(whole, buf)?;
            if written > 0 {
                return Ok(written);
            }
        }
        Ok(0)
    }

    /// Converts `units` to UTF-8 at the front of `buf`, keeping the bytes
    /// that do not fit for the next read, and returns the count written.
    fn encode(&mut self, units: &[u16], buf: &mut [u8]) -> io::Result<usize> {
        let mut written = 0;
        for c in char::decode_utf16(units.iter().copied()) {
            let c = c.map_err(|_| UNPAIRED_SURROGATE)?;
            let mut utf8 = [0; 4];
            let bytes = c.encode_utf8(&mut utf8).as_bytes();
            let free = buf.get_mut(written..).unwrap_or_default();
            let n = bytes.len().min(free.len());
            if let (Some(dst), Some(src)) = (free.get_mut(..n), bytes.get(..n))
            {
                dst.copy_from_slice(src);
            }
            written += n;
            // Only the last character can overflow; see `read_console`.
            if let Some(rest) = bytes.get(n..).filter(|rest| !rest.is_empty()) {
                debug_assert_eq!(self.pending_len, 0);
                self.pending_len = rest.len();
                if let Some(dst) = self.pending.get_mut(..rest.len()) {
                    dst.copy_from_slice(rest);
                }
            }
        }
        Ok(written)
    }

    /// Moves as many pending bytes as fit to the front of `buf`.
    fn take_pending(&mut self, buf: &mut [u8]) -> usize {
        let n = self.pending_len.min(buf.len());
        if let (Some(dst), Some(src)) =
            (buf.get_mut(..n), self.pending.get(..n))
        {
            dst.copy_from_slice(src);
        }
        self.pending.copy_within(n.., 0);
        self.pending_len -= n;
        n
    }
}

/// std's error for console input that is not valid UTF-16.
#[cfg(all(feature = "stdio", feature = "io"))]
const UNPAIRED_SURROGATE: io::Error = io::const_error!(
    io::ErrorKind::InvalidData,
    "Windows stdin in console mode does not support non-UTF-16 input; \
     encountered unpaired surrogate",
);

/// Reads UTF-16 units from the console into `units`, returning how many.
/// Like std, it returns early at a Ctrl-Z, which it drops, and retries when
/// Ctrl-C or Ctrl-Break cancels the read.
#[cfg(all(feature = "stdio", feature = "io"))]
fn read_console_units(handle: HANDLE, units: &mut [u16]) -> io::Result<usize> {
    const CTRL_Z: u16 = 0x1A;
    /// The size of the control block, 16 bytes.
    #[allow(clippy::cast_possible_truncation, reason = "a small size")]
    const CONTROL_SIZE: u32 = size_of::<CONSOLE_READCONSOLE_CONTROL>() as u32;
    let control = CONSOLE_READCONSOLE_CONTROL {
        nLength: CONTROL_SIZE,
        nInitialChars: 0,
        dwCtrlWakeupMask: 1 << CTRL_Z,
        dwControlKeyState: 0,
    };
    let len = u32::try_from(units.len()).unwrap_or(u32::MAX);
    let mut count = MaybeUninit::<u32>::uninit();
    // Repeats only when Ctrl-C or Ctrl-Break cancelled the read before any
    // input; each such key press ends one pass, so the loop ends once the
    // user types input or the console closes.
    let read = loop {
        // SAFETY: `units` is writable for `len` units, `count` for a `u32`,
        // and `control` is a valid control block; the read is synchronous.
        let ok = unsafe {
            SetLastError(0);
            ReadConsoleW(
                handle,
                units.as_mut_ptr().cast(),
                len,
                count.as_mut_ptr(),
                &raw const control,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a successful `ReadConsoleW` stores the count it read.
        let read = unsafe { count.assume_init() };
        // SAFETY: `GetLastError` has no preconditions.
        if read != 0 || unsafe { GetLastError() } != ERROR_OPERATION_ABORTED {
            break read;
        }
    };
    let mut read = (read as usize).min(units.len());
    if read > 0 && units.get(read - 1) == Some(&CTRL_Z) {
        read -= 1;
    }
    Ok(read)
}

/// Whether `stream` is a terminal.
#[cfg(all(feature = "stdio", feature = "io"))]
pub(crate) fn is_terminal(stream: Stream) -> bool {
    super::terminal::is_terminal(raw_handle(stream))
}
