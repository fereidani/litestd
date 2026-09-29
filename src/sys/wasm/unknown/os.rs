//! The process of a module without an OS, as std has it there: no errors
//! to read, and no status or id.

#[cfg(feature = "io")]
use core::fmt;

#[cfg(feature = "io")]
use crate::io::ErrorKind;

/// The last OS error, which there never is: 0, as in std.
#[cfg(feature = "io")]
pub(crate) fn errno() -> i32 {
    0
}

/// There are no signals to interrupt a call.
#[cfg(feature = "io")]
pub(crate) fn is_interrupted(_code: i32) -> bool {
    false
}

/// No OS error code has a kind, as in std.
#[cfg(feature = "io")]
pub(crate) fn decode_error_kind(_code: i32) -> ErrorKind {
    ErrorKind::Uncategorized
}

/// Writes std's message for any code there.
#[cfg(feature = "io")]
pub(crate) fn fmt_error_message(
    _code: i32,
    f: &mut fmt::Formatter<'_>,
) -> fmt::Result {
    f.write_str("operation successful")
}

/// Ends the module with a trap.
// Test builds link std and compile out the panic handler; with no module
// feature on, nothing else calls this.
#[cfg_attr(any(test, feature = "test-with-std"), allow(dead_code))]
pub(crate) fn abort() -> ! {
    core::arch::wasm32::unreachable()
}

/// Ends the module: there is no one to report `code` to, and std aborts
/// too.
#[cfg(feature = "process")]
pub(crate) fn exit(_code: i32) -> ! {
    abort()
}

/// Panics as std does: there are no process ids.
#[cfg(feature = "process")]
#[cold]
#[track_caller]
#[allow(clippy::panic, reason = "std panics here")]
pub(crate) fn id() -> u32 {
    panic!("no pids on this platform")
}
