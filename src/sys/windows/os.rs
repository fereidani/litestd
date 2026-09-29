//! OS error codes, synchronous transfers through handles, the UTF-16
//! strings of the API, process control, and timeouts.

#[cfg(any(feature = "command", feature = "env"))]
pub(crate) mod environ;
#[cfg(any(
    feature = "io",
    feature = "stdio",
    all(
        feature = "panic-location",
        not(any(
            test,
            feature = "test-with-std",
            feature = "custom-panic-handler"
        ))
    )
))]
pub(crate) mod handle;
#[cfg(any(feature = "command", feature = "env", feature = "fs"))]
pub(crate) mod wide;

#[cfg(any(feature = "net", feature = "sync", feature = "thread"))]
use core::time::Duration;
#[cfg(feature = "io")]
use core::{cell::Cell, fmt, ptr};

#[cfg(feature = "process")]
use windows_sys::Win32::System::Threading::{ExitProcess, GetCurrentProcessId};
#[cfg(feature = "io")]
use windows_sys::{
    Win32::{
        Foundation::{
            DNS_ERROR_RECORD_TIMED_OUT, ERROR_ACCESS_DENIED,
            ERROR_ALREADY_EXISTS, ERROR_BAD_NET_NAME, ERROR_BAD_NETPATH,
            ERROR_BAD_PATHNAME, ERROR_BROKEN_PIPE, ERROR_BUSY,
            ERROR_CALL_NOT_IMPLEMENTED, ERROR_CANT_RESOLVE_FILENAME,
            ERROR_COUNTER_TIMEOUT, ERROR_CTX_CLIENT_QUERY_TIMEOUT,
            ERROR_CTX_MODEM_RESPONSE_TIMEOUT, ERROR_DIR_NOT_EMPTY,
            ERROR_DIRECTORY, ERROR_DIRECTORY_NOT_SUPPORTED, ERROR_DISK_FULL,
            ERROR_DISK_QUOTA_EXCEEDED, ERROR_DRIVER_CANCEL_TIMEOUT,
            ERROR_DS_TIMELIMIT_EXCEEDED, ERROR_FILE_EXISTS,
            ERROR_FILE_NOT_FOUND, ERROR_FILE_TOO_LARGE,
            ERROR_FILENAME_EXCED_RANGE, ERROR_HANDLE_DISK_FULL,
            ERROR_HOST_UNREACHABLE, ERROR_INVALID_DRIVE, ERROR_INVALID_NAME,
            ERROR_INVALID_PARAMETER, ERROR_IPSEC_IKE_TIMED_OUT,
            ERROR_NEGATIVE_SEEK, ERROR_NETWORK_UNREACHABLE, ERROR_NO_DATA,
            ERROR_NOT_ENOUGH_MEMORY, ERROR_NOT_SAME_DEVICE,
            ERROR_OPERATION_ABORTED, ERROR_OUTOFMEMORY, ERROR_PATH_NOT_FOUND,
            ERROR_POSSIBLE_DEADLOCK, ERROR_RESOURCE_CALL_TIMED_OUT,
            ERROR_RUNLEVEL_SWITCH_AGENT_TIMEOUT, ERROR_RUNLEVEL_SWITCH_TIMEOUT,
            ERROR_SEEK_ON_DEVICE, ERROR_SEM_TIMEOUT,
            ERROR_SERVICE_REQUEST_TIMEOUT, ERROR_TIMEOUT, ERROR_TOO_MANY_LINKS,
            ERROR_TOO_MANY_OPEN_FILES, ERROR_WRITE_PROTECT, GetLastError,
            HMODULE, NTSTATUS, RtlNtStatusToDosError, WAIT_TIMEOUT,
        },
        System::{
            Diagnostics::Debug::{
                FORMAT_MESSAGE_FROM_HMODULE, FORMAT_MESSAGE_FROM_SYSTEM,
                FORMAT_MESSAGE_IGNORE_INSERTS, FormatMessageW,
            },
            LibraryLoader::GetModuleHandleW,
        },
    },
    core::w,
};

#[cfg(feature = "io")]
use crate::io::{self, ErrorKind};

/// Returns the `i32` with the bits of a Windows error code, as std stores
/// it.
#[cfg(any(
    feature = "io",
    feature = "stdio",
    all(
        feature = "panic-location",
        not(any(
            test,
            feature = "test-with-std",
            feature = "custom-panic-handler"
        ))
    )
))]
pub(crate) const fn os_code(code: u32) -> i32 {
    #[allow(clippy::cast_possible_wrap)]
    let code = code as i32;
    code
}

/// Returns the calling thread's last error code.
#[cfg(feature = "io")]
pub(crate) fn last_error() -> u32 {
    // SAFETY: `GetLastError` has no preconditions.
    unsafe { GetLastError() }
}

/// Returns the calling thread's last error code as std stores it.
#[cfg(feature = "io")]
pub(crate) fn errno() -> i32 {
    os_code(last_error())
}

/// Returns an error for a Windows error code.
#[cfg(feature = "io")]
pub(crate) fn win_error(code: u32) -> io::Error {
    io::Error::from_raw_os_error(os_code(code))
}

/// Converts the `BOOL` result of a Win32 call to an `io::Result`.
#[cfg(any(feature = "command", feature = "env", feature = "fs"))]
pub(crate) fn cvt(result: i32) -> io::Result<()> {
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Returns the error for a failed `NTSTATUS`.
#[cfg(feature = "io")]
pub(crate) fn nt_error(status: NTSTATUS) -> io::Error {
    // SAFETY: `RtlNtStatusToDosError` has no preconditions.
    win_error(unsafe { RtlNtStatusToDosError(status) })
}

/// Whether the error code means that the call was interrupted, which no
/// Windows error code does.
#[cfg(feature = "io")]
#[inline]
pub(crate) const fn is_interrupted(_code: i32) -> bool {
    false
}

/// Classifies a Windows error code the way std does. Its search is large, so
/// every caller shares one copy.
#[cfg(feature = "io")]
#[inline(never)]
pub(crate) const fn decode_error_kind(code: i32) -> ErrorKind {
    #[allow(clippy::cast_sign_loss)]
    match code as u32 {
        ERROR_ACCESS_DENIED | wsa::EACCES => ErrorKind::PermissionDenied,
        ERROR_ALREADY_EXISTS | ERROR_FILE_EXISTS => ErrorKind::AlreadyExists,
        ERROR_BROKEN_PIPE | ERROR_NO_DATA | wsa::ESHUTDOWN => {
            ErrorKind::BrokenPipe
        }
        ERROR_BUSY => ErrorKind::ResourceBusy,
        ERROR_CALL_NOT_IMPLEMENTED => ErrorKind::Unsupported,
        ERROR_CANT_RESOLVE_FILENAME => ErrorKind::FilesystemLoop,
        ERROR_DIR_NOT_EMPTY => ErrorKind::DirectoryNotEmpty,
        ERROR_DIRECTORY => ErrorKind::NotADirectory,
        ERROR_DIRECTORY_NOT_SUPPORTED => ErrorKind::IsADirectory,
        ERROR_DISK_FULL | ERROR_HANDLE_DISK_FULL => ErrorKind::StorageFull,
        ERROR_DISK_QUOTA_EXCEEDED | wsa::EDQUOT => ErrorKind::QuotaExceeded,
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_INVALID_DRIVE
        | ERROR_BAD_NETPATH | ERROR_BAD_NET_NAME => ErrorKind::NotFound,
        ERROR_FILE_TOO_LARGE => ErrorKind::FileTooLarge,
        ERROR_FILENAME_EXCED_RANGE
        | ERROR_INVALID_NAME
        | ERROR_BAD_PATHNAME => ErrorKind::InvalidFilename,
        ERROR_HOST_UNREACHABLE | wsa::EHOSTUNREACH => {
            ErrorKind::HostUnreachable
        }
        ERROR_INVALID_PARAMETER | ERROR_NEGATIVE_SEEK | wsa::EINVAL => {
            ErrorKind::InvalidInput
        }
        ERROR_NETWORK_UNREACHABLE | wsa::ENETUNREACH => {
            ErrorKind::NetworkUnreachable
        }
        ERROR_NOT_ENOUGH_MEMORY | ERROR_OUTOFMEMORY => ErrorKind::OutOfMemory,
        ERROR_NOT_SAME_DEVICE => ErrorKind::CrossesDevices,
        ERROR_SEM_TIMEOUT
        | WAIT_TIMEOUT
        | ERROR_DRIVER_CANCEL_TIMEOUT
        | ERROR_OPERATION_ABORTED
        | ERROR_SERVICE_REQUEST_TIMEOUT
        | ERROR_COUNTER_TIMEOUT
        | ERROR_TIMEOUT
        | ERROR_RESOURCE_CALL_TIMED_OUT
        | ERROR_CTX_MODEM_RESPONSE_TIMEOUT
        | ERROR_CTX_CLIENT_QUERY_TIMEOUT
        | FRS_ERR_SYSVOL_POPULATE_TIMEOUT
        | ERROR_DS_TIMELIMIT_EXCEEDED
        | DNS_ERROR_RECORD_TIMED_OUT
        | ERROR_IPSEC_IKE_TIMED_OUT
        | ERROR_RUNLEVEL_SWITCH_TIMEOUT
        | ERROR_RUNLEVEL_SWITCH_AGENT_TIMEOUT
        | wsa::ETIMEDOUT => ErrorKind::TimedOut,
        ERROR_POSSIBLE_DEADLOCK => ErrorKind::Deadlock,
        ERROR_SEEK_ON_DEVICE => ErrorKind::NotSeekable,
        ERROR_TOO_MANY_LINKS => ErrorKind::TooManyLinks,
        ERROR_TOO_MANY_OPEN_FILES | wsa::EMFILE => ErrorKind::TooManyOpenFiles,
        ERROR_WRITE_PROTECT => ErrorKind::ReadOnlyFilesystem,
        wsa::EADDRINUSE => ErrorKind::AddrInUse,
        wsa::EADDRNOTAVAIL => ErrorKind::AddrNotAvailable,
        wsa::ECONNABORTED => ErrorKind::ConnectionAborted,
        wsa::ECONNREFUSED => ErrorKind::ConnectionRefused,
        wsa::ECONNRESET => ErrorKind::ConnectionReset,
        wsa::ENOTCONN => ErrorKind::NotConnected,
        wsa::EWOULDBLOCK => ErrorKind::WouldBlock,
        wsa::ENETDOWN => ErrorKind::NetworkDown,
        _ => ErrorKind::Uncategorized,
    }
}

/// `FRS_ERR_SYSVOL_POPULATE_TIMEOUT`, which windows-sys types as `i32`.
#[cfg(feature = "io")]
const FRS_ERR_SYSVOL_POPULATE_TIMEOUT: u32 = 8014;

/// The Winsock error codes std classifies, from `winerror.h`, to avoid
/// windows-sys's large `WinSock` module for constants alone.
#[cfg(feature = "io")]
mod wsa {
    pub(super) const EACCES: u32 = 10013;
    pub(super) const EINVAL: u32 = 10022;
    pub(super) const EMFILE: u32 = 10024;
    pub(super) const EWOULDBLOCK: u32 = 10035;
    pub(super) const EADDRINUSE: u32 = 10048;
    pub(super) const EADDRNOTAVAIL: u32 = 10049;
    pub(super) const ENETDOWN: u32 = 10050;
    pub(super) const ENETUNREACH: u32 = 10051;
    pub(super) const ECONNABORTED: u32 = 10053;
    pub(super) const ECONNRESET: u32 = 10054;
    pub(super) const ENOTCONN: u32 = 10057;
    pub(super) const ESHUTDOWN: u32 = 10058;
    pub(super) const ETIMEDOUT: u32 = 10060;
    pub(super) const ECONNREFUSED: u32 = 10061;
    pub(super) const EHOSTUNREACH: u32 = 10065;
    pub(super) const EDQUOT: u32 = 10069;
}

/// Writes the system's description of `code`, as `FormatMessageW` gives it,
/// with std's text for the codes it has none for.
#[cfg(feature = "io")]
pub(crate) fn fmt_error_message(
    code: i32,
    f: &mut fmt::Formatter<'_>,
) -> fmt::Result {
    let mut buf = MessageBuf([0; 3 * MESSAGE_UNITS as usize]);
    let (code, len) = buf.format(code);
    if len == 0 {
        let err = errno();
        return write!(
            f,
            "OS Error {code} (FormatMessageW() returned error {err})"
        );
    }
    match buf.decode(len) {
        Some(text) => f.write_str(text),
        None => write!(
            f,
            "OS Error {code} (FormatMessageW() returned invalid UTF-16)"
        ),
    }
}

/// The most UTF-16 units of a message, the same as std's.
#[cfg(feature = "io")]
const MESSAGE_UNITS: u32 = 2048;

/// Room for a message of up to [`MESSAGE_UNITS`] UTF-16 units, at its back,
/// and for their UTF-8, at most three bytes for each, at its front. So the
/// conversion works in place: the bytes of the first `k` units end at
/// `3 * k`, never past unit `k`, at `MESSAGE_UNITS + 2 * k`.
#[cfg(feature = "io")]
#[repr(C, align(2))]
struct MessageBuf([u8; 3 * MESSAGE_UNITS as usize]);

#[cfg(feature = "io")]
impl MessageBuf {
    /// Writes the message for `code` with `FormatMessageW`. Returns the code
    /// it looked up, without `FACILITY_NT_BIT` if `ntdll` has the message,
    /// and the number of units written, 0 on failure.
    fn format(&mut self, mut code: i32) -> (i32, u32) {
        /// Marks an `NTSTATUS` wrapped in an `HRESULT`; `ntdll` holds the
        /// messages of those.
        const FACILITY_NT_BIT: i32 = 0x1000_0000;
        let mut flags =
            FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS;
        let mut module: HMODULE = ptr::null_mut();
        if code & FACILITY_NT_BIT != 0 {
            // SAFETY: the name is NUL-terminated UTF-16; the handle is not
            // reference counted, and `ntdll` stays loaded for the process's
            // life.
            module = unsafe { GetModuleHandleW(w!("ntdll.dll")) };
            if !module.is_null() {
                code ^= FACILITY_NT_BIT;
                flags |= FORMAT_MESSAGE_FROM_HMODULE;
            }
        }
        let units = self.0.as_mut_ptr().wrapping_add(MESSAGE_UNITS as usize);
        // SAFETY: `units` is valid for writes of `MESSAGE_UNITS` units, the
        // back two thirds of the buffer, and aligned for them. No arguments
        // are read with `FORMAT_MESSAGE_IGNORE_INSERTS`, and `module` is
        // null or a loaded module, as `flags` says. The cast restores the
        // `u32` code.
        #[allow(clippy::cast_sign_loss)]
        let len = unsafe {
            FormatMessageW(
                flags,
                module.cast_const(),
                code as u32,
                0,
                units.cast(),
                MESSAGE_UNITS,
                ptr::null(),
            )
        };
        (code, len)
    }

    /// Converts the first `len` units, fewer than [`MESSAGE_UNITS`], to
    /// UTF-8 at the front, and returns it without its trailing ASCII
    /// whitespace, which std trims too. Returns `None` if the units are not
    /// valid UTF-16.
    fn decode(&mut self, len: u32) -> Option<&str> {
        let cells = Cell::from_mut(&mut self.0[..]).as_slice_of_cells();
        let unit = |k: u32| {
            let at = (MESSAGE_UNITS + 2 * k) as usize;
            let byte = |i: usize| cells.get(i).map_or(0, Cell::get);
            u16::from_ne_bytes([byte(at), byte(at + 1)])
        };
        let (mut written, mut end) = (0, 0);
        for c in char::decode_utf16((0..len).map(unit)) {
            let c = c.ok()?;
            for &byte in c.encode_utf8(&mut [0; 4]).as_bytes() {
                if let Some(cell) = cells.get(written) {
                    cell.set(byte);
                    written += 1;
                }
            }
            if !c.is_ascii_whitespace() {
                end = written;
            }
        }
        // Whole encoded characters one after another: one valid chunk.
        let text = self.0.get(..end).unwrap_or_default();
        Some(text.utf8_chunks().next().map_or("", |chunk| chunk.valid()))
    }
}

/// Terminates the process abnormally with `__fastfail`, which exception
/// handlers cannot intercept, without running destructors or exit handlers.
// Test builds link std and compile out the panic handler and the
// personality routine; with no module feature on, nothing else calls this.
#[cfg_attr(any(test, feature = "test-with-std"), allow(dead_code))]
pub(crate) fn abort() -> ! {
    /// `FAST_FAIL_FATAL_APP_EXIT` from `winnt.h`.
    const FAST_FAIL_FATAL_APP_EXIT: u32 = 7;
    // SAFETY: the `__fastfail` sequence raises a non-continuable exception
    // that terminates the process; it never returns and touches no memory.
    unsafe {
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        core::arch::asm!(
            "int 0x29",
            in("ecx") FAST_FAIL_FATAL_APP_EXIT,
            options(noreturn, nostack),
        );
        #[cfg(any(target_arch = "aarch64", target_arch = "arm64ec"))]
        core::arch::asm!(
            "brk 0xF003",
            in("x0") FAST_FAIL_FATAL_APP_EXIT,
            options(noreturn, nostack),
        );
    }
}

/// Makes the linker pick up the unwinder that `core` and `alloc` reference:
/// nothing to do, as windows-gnu links its own and MSVC's is in the C runtime.
#[cfg(not(any(test, feature = "test-with-std")))]
pub(crate) const fn reference_unwinder() {}

/// Terminates the process with `code`, after the loader notified every DLL.
#[cfg(feature = "process")]
pub(crate) fn exit(code: i32) -> ! {
    // Windows exit codes are `u32` with the same bits, as in std.
    #[allow(clippy::cast_sign_loss)]
    let code = code as u32;
    // SAFETY: `ExitProcess` has no preconditions and does not return.
    unsafe { ExitProcess(code) }
}

/// Returns the id of the calling process.
#[cfg(feature = "process")]
pub(crate) fn id() -> u32 {
    // SAFETY: `GetCurrentProcessId` has no preconditions.
    unsafe { GetCurrentProcessId() }
}

/// Converts a timeout to milliseconds, rounded up so a wait never ends
/// early, and at most `max`.
#[cfg(any(feature = "net", feature = "sync", feature = "thread"))]
pub(crate) fn timeout_ms(timeout: Duration, max: u32) -> u32 {
    let ms = timeout
        .as_secs()
        .saturating_mul(1000)
        .saturating_add(u64::from(timeout.subsec_nanos().div_ceil(1_000_000)));
    u32::try_from(ms).map_or(max, |ms| ms.min(max))
}
