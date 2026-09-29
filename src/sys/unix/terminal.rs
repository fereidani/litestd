//! Terminal detection for `io::IsTerminal`.

/// Whether `fd` refers to a terminal. An invalid descriptor is not one.
pub(crate) fn is_terminal(fd: libc::c_int) -> bool {
    // SAFETY: `isatty` has no preconditions; it only queries `fd`.
    unsafe { libc::isatty(fd) != 0 }
}
