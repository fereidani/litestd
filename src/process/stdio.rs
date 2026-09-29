//! [`Stdio`], the configuration of a child's standard streams.

use core::fmt;

use super::{ChildStderr, ChildStdin, ChildStdout};
#[cfg(feature = "fs")]
use crate::fs;
use crate::{io, sys::process as imp};

/// Describes what to do with a standard I/O stream for a child process when
/// passed to the [`stdin`], [`stdout`], and [`stderr`] methods of
/// [`Command`](super::Command).
///
/// [`stdin`]: super::Command::stdin
/// [`stdout`]: super::Command::stdout
/// [`stderr`]: super::Command::stderr
pub struct Stdio(pub(crate) imp::Stdio);

impl Stdio {
    /// A new pipe should be arranged to connect the parent and child
    /// processes.
    ///
    /// Writing more than a pipe buffer's worth of input to stdin without
    /// also reading stdout and stderr at the same time may cause a deadlock.
    #[must_use]
    #[allow(clippy::missing_const_for_fn, reason = "not `const` in std")]
    pub fn piped() -> Self {
        Self(imp::Stdio::MakePipe)
    }

    /// The child inherits from the corresponding parent descriptor.
    #[must_use]
    #[allow(clippy::missing_const_for_fn, reason = "not `const` in std")]
    pub fn inherit() -> Self {
        Self(imp::Stdio::Inherit)
    }

    /// This stream will be ignored. This is the equivalent of attaching the
    /// stream to `/dev/null`.
    #[must_use]
    #[allow(clippy::missing_const_for_fn, reason = "not `const` in std")]
    pub fn null() -> Self {
        Self(imp::Stdio::Null)
    }
}

impl fmt::Debug for Stdio {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stdio").finish_non_exhaustive()
    }
}

impl From<ChildStdin> for Stdio {
    /// Converts a [`ChildStdin`] into a [`Stdio`], so that another child
    /// writes to the first one's input.
    fn from(child: ChildStdin) -> Self {
        Self(imp::Stdio::from(child.inner))
    }
}

impl From<ChildStdout> for Stdio {
    /// Converts a [`ChildStdout`] into a [`Stdio`], so that another child
    /// reads the first one's output.
    fn from(child: ChildStdout) -> Self {
        Self(imp::Stdio::from(child.inner))
    }
}

impl From<ChildStderr> for Stdio {
    /// Converts a [`ChildStderr`] into a [`Stdio`].
    fn from(child: ChildStderr) -> Self {
        Self(imp::Stdio::from(child.inner))
    }
}

impl From<io::PipeReader> for Stdio {
    /// Makes the read end of an [`io::pipe`] the child's stream.
    fn from(pipe: io::PipeReader) -> Self {
        Self(imp::Stdio::from(pipe.0))
    }
}

impl From<io::PipeWriter> for Stdio {
    /// Makes the write end of an [`io::pipe`] the child's stream.
    fn from(pipe: io::PipeWriter) -> Self {
        Self(imp::Stdio::from(pipe.0))
    }
}

#[cfg(feature = "stdio")]
impl From<io::Stdout> for Stdio {
    /// Redirects the child's stream to this process's stdout.
    fn from(_: io::Stdout) -> Self {
        Self(imp::Stdio::STDOUT)
    }
}

#[cfg(feature = "stdio")]
impl From<io::Stderr> for Stdio {
    /// Redirects the child's stream to this process's stderr.
    fn from(_: io::Stderr) -> Self {
        Self(imp::Stdio::STDERR)
    }
}

#[cfg(feature = "fs")]
impl From<fs::File> for Stdio {
    /// Converts a [`File`](fs::File) into a [`Stdio`], which the child uses
    /// as the stream.
    fn from(file: fs::File) -> Self {
        Self(imp::Stdio::from(file))
    }
}
