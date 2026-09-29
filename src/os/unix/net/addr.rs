//! [`SocketAddr`], the address of a Unix domain socket.

use crate::{
    ffi::OsStr,
    fmt, io,
    os::unix::ffi::OsStrExt,
    path::Path,
    sys::net::{AddrKind, UnixAddr},
};

/// An address associated with a Unix socket.
#[derive(Clone)]
pub struct SocketAddr {
    pub(crate) inner: UnixAddr,
}

impl SocketAddr {
    /// Constructs a `SocketAddr` with the family `AF_UNIX` and the provided
    /// path.
    ///
    /// # Errors
    ///
    /// Fails if the path is longer than `SUN_LEN` or contains a NUL byte.
    pub fn from_pathname<P>(path: P) -> io::Result<Self>
    where
        P: AsRef<Path>,
    {
        Self::from_path(path.as_ref())
    }

    pub(super) fn from_path(path: &Path) -> io::Result<Self> {
        let path = path.as_os_str().as_bytes();
        UnixAddr::from_pathname(path).map(|inner| Self { inner })
    }

    /// Returns `true` if the address is unnamed.
    #[must_use]
    pub fn is_unnamed(&self) -> bool {
        matches!(self.inner.kind(), AddrKind::Unnamed)
    }

    /// Returns the contents of this address if it is a `pathname` address.
    #[must_use]
    pub fn as_pathname(&self) -> Option<&Path> {
        if let AddrKind::Pathname(path) = self.inner.kind() {
            Some(Path::new(OsStr::from_bytes(path)))
        } else {
            None
        }
    }
}

impl fmt::Debug for SocketAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.inner.kind() {
            AddrKind::Unnamed => f.write_str("(unnamed)"),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            AddrKind::Abstract(name) => {
                write!(f, "{:?} (abstract)", EscapedBytes(name))
            }
            AddrKind::Pathname(path) => {
                write!(f, "{:?} (pathname)", Path::new(OsStr::from_bytes(path)))
            }
        }
    }
}

/// Formats bytes the way std formats an abstract name: in quotes, UTF-8 as
/// text, with Rust's escapes for quotes, backslashes and control characters,
/// and `\xNN` for the other bytes.
#[cfg(any(target_os = "linux", target_os = "android"))]
struct EscapedBytes<'a>(&'a [u8]);

#[cfg(any(target_os = "linux", target_os = "android"))]
impl fmt::Debug for EscapedBytes<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("\"")?;
        for chunk in self.0.utf8_chunks() {
            for c in chunk.valid().chars() {
                match u8::try_from(c) {
                    Ok(0) => f.write_str("\\0")?,
                    Ok(b) if b.is_ascii() => write!(f, "{}", b.escape_ascii())?,
                    _ => write!(f, "{}", c.escape_debug())?,
                }
            }
            write!(f, "{}", chunk.invalid().escape_ascii())?;
        }
        f.write_str("\"")
    }
}
