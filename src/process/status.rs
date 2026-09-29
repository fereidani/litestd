//! [`ExitStatus`], how a child process ended.

use core::fmt;

use crate::sys::process as imp;

/// Describes the result of a process after it has terminated.
///
/// It represents every possible disposition of a process: on Unix it is the
/// wait status, not simply the value passed to `exit`, and
/// `os::unix::process::ExitStatusExt` reads its other parts. Print it with
/// `Display` to report how a process failed.
///
/// Unlike [`ExitCode`](super::ExitCode), which the current process returns,
/// it describes a child that has terminated.
///
/// The default value indicates successful completion.
#[derive(PartialEq, Eq, Clone, Copy, Debug, Default)]
pub struct ExitStatus(pub(crate) imp::ExitStatus);

impl ExitStatus {
    /// Was termination successful? Signal termination is not considered a
    /// success, and success is defined as a zero exit status.
    #[must_use]
    #[allow(clippy::missing_const_for_fn, reason = "not `const` in std")]
    pub fn success(&self) -> bool {
        self.0.success()
    }

    /// Returns the exit code of the process, if any.
    ///
    /// On Unix this is the value passed to `exit`, truncated to 8 bits, and
    /// `None` if a signal terminated the process.
    #[must_use]
    #[allow(clippy::missing_const_for_fn, reason = "not `const` in std")]
    pub fn code(&self) -> Option<i32> {
        self.0.code()
    }
}

impl fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}
