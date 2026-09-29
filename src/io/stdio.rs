//! [`IsTerminal`], and with `stdio` the standard stream handles.

#[cfg(feature = "stdio")]
mod handles;

#[cfg(feature = "stdio")]
pub use self::handles::{
    Stderr, StderrLock, Stdin, StdinLock, Stdout, StdoutLock, stderr, stdin,
    stdout,
};
#[cfg(feature = "fs")]
use crate::fs;
#[cfg(any(unix, target_os = "wasi"))]
use crate::os::fd::{AsRawFd, BorrowedFd, OwnedFd};
#[cfg(windows)]
use crate::os::windows::io::{AsRawHandle, BorrowedHandle, OwnedHandle};

mod private {
    /// Seals `IsTerminal`, as in std.
    pub trait Sealed {}
}

/// Trait to determine if a descriptor/handle refers to a terminal/tty.
///
/// This trait is sealed: it cannot be implemented outside litestd.
pub trait IsTerminal: private::Sealed {
    /// Returns `true` if the descriptor/handle refers to a terminal/tty.
    ///
    /// Anything else, an invalid descriptor included, returns `false`. On
    /// Windows, besides consoles, this recognizes the pipes of msys and
    /// cygwin pseudo-terminals by their names, which start with `msys-` or
    /// `cygwin-` and contain `-pty`, as std does.
    #[doc(alias = "isatty", alias = "atty")]
    fn is_terminal(&self) -> bool;
}

/// Implements [`IsTerminal`] for types with a raw descriptor or handle.
#[cfg(any(unix, windows, target_os = "wasi", feature = "fs"))]
macro_rules! impl_is_terminal {
    ($($ty:ty),* $(,)?) => {$(
        impl private::Sealed for $ty {}

        impl IsTerminal for $ty {
            #[inline]
            fn is_terminal(&self) -> bool {
                is_terminal_raw(self)
            }
        }
    )*};
}

/// Whether the descriptor of `io` is a terminal.
#[cfg(any(unix, target_os = "wasi"))]
#[inline]
fn is_terminal_raw(io: &impl AsRawFd) -> bool {
    crate::sys::terminal::is_terminal(io.as_raw_fd())
}

/// Whether the handle of `io` is a terminal.
#[cfg(windows)]
#[inline]
fn is_terminal_raw(io: &impl AsRawHandle) -> bool {
    crate::sys::terminal::is_terminal(io.as_raw_handle())
}

/// Without an OS, nothing is a terminal.
#[cfg(all(not(any(unix, windows, target_os = "wasi")), feature = "fs"))]
#[inline]
const fn is_terminal_raw<T>(_: &T) -> bool {
    false
}

#[cfg(feature = "fs")]
impl_is_terminal!(fs::File);
#[cfg(any(unix, target_os = "wasi"))]
impl_is_terminal!(BorrowedFd<'_>, OwnedFd);
#[cfg(windows)]
impl_is_terminal!(BorrowedHandle<'_>, OwnedHandle);
