//! [`ErrorKind`] and the description of each kind.

use core::fmt;

/// Defines `ErrorKind`, and `as_str` and `from_bits` from the same list of
/// kinds, so that neither can miss one. The kinds are in declaration order, so
/// that `from_bits` compiles to one comparison.
macro_rules! error_kinds {
    ($($(#[$meta:meta])* $kind:ident => $text:literal,)*) => {
        /// A list specifying general categories of I/O error.
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        #[non_exhaustive]
        pub enum ErrorKind {
            $($(#[$meta])* $kind,)*
        }

        impl ErrorKind {
            pub(super) const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$kind => $text,)*
                }
            }

            /// The kind whose discriminant is `bits`, or `Uncategorized`.
            #[cfg(target_pointer_width = "64")]
            #[inline]
            pub(super) const fn from_bits(bits: u32) -> Self {
                $(
                    if bits == Self::$kind as u32 {
                        return Self::$kind;
                    }
                )*
                Self::Uncategorized
            }
        }
    };
}

error_kinds! {
    /// An entity was not found, often a file.
    NotFound => "entity not found",
    /// The operation lacked the necessary privileges to complete.
    PermissionDenied => "permission denied",
    /// The connection was refused by the remote server.
    ConnectionRefused => "connection refused",
    /// The connection was reset by the remote server.
    ConnectionReset => "connection reset",
    /// The remote host is not reachable.
    HostUnreachable => "host unreachable",
    /// The network containing the remote host is not reachable.
    NetworkUnreachable => "network unreachable",
    /// The connection was aborted (terminated) by the remote server.
    ConnectionAborted => "connection aborted",
    /// The network operation failed because it was not connected yet.
    NotConnected => "not connected",
    /// A socket address could not be bound because it is already in use.
    AddrInUse => "address in use",
    /// A nonexistent interface was requested or the address was not local.
    AddrNotAvailable => "address not available",
    /// The system's networking is down.
    NetworkDown => "network down",
    /// The operation failed because a pipe was closed.
    BrokenPipe => "broken pipe",
    /// An entity already exists, often a file.
    AlreadyExists => "entity already exists",
    /// The operation needs to block to complete, but was asked not to.
    WouldBlock => "operation would block",
    /// A filesystem object is, unexpectedly, not a directory.
    NotADirectory => "not a directory",
    /// The filesystem object is, unexpectedly, a directory.
    IsADirectory => "is a directory",
    /// A non-empty directory was specified where an empty one was expected.
    DirectoryNotEmpty => "directory not empty",
    /// A write was attempted on a read-only filesystem or storage medium.
    ReadOnlyFilesystem => "read-only filesystem or storage medium",
    /// A symbolic link loop or too many of them on one path. Unstable in std,
    /// whose OS errors still report it; hidden, like std's.
    #[doc(hidden)]
    FilesystemLoop => "filesystem loop or indirection limit (e.g. symlink loop)",
    /// Stale network file handle.
    StaleNetworkFileHandle => "stale network file handle",
    /// A parameter was incorrect.
    InvalidInput => "invalid input parameter",
    /// Data not valid for the operation were encountered.
    InvalidData => "invalid data",
    /// The I/O operation's timeout expired, causing it to be canceled.
    TimedOut => "timed out",
    /// A call to `write` returned `Ok(0)`, so the operation could not finish.
    WriteZero => "write zero",
    /// The underlying storage is full.
    StorageFull => "no storage space",
    /// Seek on unseekable file.
    NotSeekable => "seek on unseekable file",
    /// Filesystem quota or some other kind of quota was exceeded.
    QuotaExceeded => "quota exceeded",
    /// File larger than allowed or supported.
    FileTooLarge => "file too large",
    /// Resource is busy.
    ResourceBusy => "resource busy",
    /// Executable file is busy.
    ExecutableFileBusy => "executable file busy",
    /// Deadlock (avoided).
    Deadlock => "deadlock",
    /// Cross-device or cross-filesystem (hard) link or rename.
    CrossesDevices => "cross-device link or rename",
    /// Too many (hard) links to the same filesystem object.
    TooManyLinks => "too many links",
    /// A filename was invalid.
    InvalidFilename => "invalid filename",
    /// Program argument list too long.
    ArgumentListTooLong => "argument list too long",
    /// This operation was interrupted.
    Interrupted => "operation interrupted",
    /// This operation is unsupported on this platform.
    Unsupported => "unsupported",
    /// An "end of file" was reached prematurely.
    UnexpectedEof => "unexpected end of file",
    /// An operation failed to allocate enough memory.
    OutOfMemory => "out of memory",
    /// The operation was started and completes later. Unstable and hidden,
    /// as `FilesystemLoop`.
    #[doc(hidden)]
    InProgress => "in progress",
    /// The process or the system has too many open files. Unstable and
    /// hidden, as `FilesystemLoop`.
    #[doc(hidden)]
    TooManyOpenFiles => "too many open files",
    /// A custom error that does not fall under any other I/O error kind.
    Other => "other error",
    /// Any I/O error from the OS that does not fit a more specific kind. Like
    /// std's, this variant is not part of the public API.
    #[doc(hidden)]
    Uncategorized => "uncategorized error",
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
