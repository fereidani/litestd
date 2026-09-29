//! Lists of paths in the form of the `PATH` variable.

use core::{error, fmt};

#[cfg(windows)]
use alloc_crate::vec::Vec;

#[cfg(windows)]
use crate::os::windows::ffi::{EncodeWide, OsStrExt, OsStringExt};
use crate::{
    ffi::{OsStr, OsString},
    path::PathBuf,
};

/// An iterator that splits an environment variable into paths according to
/// platform-specific conventions.
///
/// The iterator element type is [`PathBuf`]. This structure is created by
/// [`split_paths`].
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct SplitPaths<'a> {
    /// The bytes after the paths yielded so far.
    #[cfg(not(windows))]
    rest: &'a [u8],
    /// The units after the paths yielded so far.
    #[cfg(windows)]
    rest: EncodeWide<'a>,
    /// Holds a path's units while its quotes are dropped.
    #[cfg(windows)]
    path: Vec<u16>,
    /// Whether the last path was yielded.
    done: bool,
}

/// Parses input according to platform conventions for the `PATH`
/// environment variable.
///
/// The separator is `:` on Unix and `;` on Windows, where double quotes
/// also enclose separators that are part of a path and are dropped. Every
/// separator ends a path, so empty input yields one empty path. Use
/// [`join_paths`] to join the paths again.
///
/// # Panics
///
/// On WebAssembly, which has no such convention, as in std.
///
/// # Examples
///
/// ```
/// use litestd::path::Path;
///
/// let paths: Vec<_> = litestd::env::split_paths("/bin:/usr/bin").collect();
/// # if cfg!(unix) {
/// assert_eq!(paths, [Path::new("/bin"), Path::new("/usr/bin")]);
/// # }
/// ```
pub fn split_paths<T: AsRef<OsStr> + ?Sized>(unparsed: &T) -> SplitPaths<'_> {
    let unparsed = unparsed.as_ref();
    #[cfg(target_family = "wasm")]
    {
        let _ = unparsed;
        no_path_lists();
    }
    #[cfg(not(target_family = "wasm"))]
    SplitPaths {
        #[cfg(not(windows))]
        rest: unparsed.as_encoded_bytes(),
        #[cfg(windows)]
        rest: unparsed.encode_wide(),
        #[cfg(windows)]
        path: Vec::new(),
        done: false,
    }
}

/// Panics as std does on WebAssembly, which has no lists of paths.
#[cfg(target_family = "wasm")]
#[cold]
#[track_caller]
#[allow(clippy::panic, reason = "std panics here")]
fn no_path_lists() -> ! {
    panic!("unsupported")
}

impl Iterator for SplitPaths<'_> {
    type Item = PathBuf;

    #[cfg(not(windows))]
    fn next(&mut self) -> Option<PathBuf> {
        if self.done {
            return None;
        }
        let rest = self.rest;
        let path = if let Some(end) = rest.iter().position(|&b| b == b':') {
            self.rest = rest.get(end + 1..).unwrap_or_default();
            rest.get(..end).unwrap_or_default()
        } else {
            self.done = true;
            rest
        };
        Some(PathBuf::from(OsStr::from_unix_bytes(path)))
    }

    #[cfg(windows)]
    fn next(&mut self) -> Option<PathBuf> {
        const QUOTE: u16 = b'"' as u16;
        const SEMICOLON: u16 = b';' as u16;
        if self.done {
            return None;
        }
        self.path.clear();
        let mut quoted = false;
        self.done = true;
        for unit in self.rest.by_ref() {
            match unit {
                QUOTE => quoted = !quoted,
                SEMICOLON if !quoted => {
                    self.done = false;
                    break;
                }
                _ => self.path.push(unit),
            }
        }
        Some(PathBuf::from(OsString::from_wide(&self.path)))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.done {
            return (0, Some(0));
        }
        // Every path but the last ends at a separator.
        #[cfg(not(windows))]
        let units = self.rest.len();
        #[cfg(windows)]
        let units = self.rest.size_hint().1.unwrap_or(usize::MAX);
        (1, units.checked_add(1))
    }
}

impl fmt::Debug for SplitPaths<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SplitPaths").finish_non_exhaustive()
    }
}

/// The separator of the paths in a list.
const SEPARATOR: &str = if cfg!(windows) { ";" } else { ":" };

/// The error type for operations on the `PATH` variable. Possibly returned
/// from [`join_paths`].
#[derive(Debug)]
pub struct JoinPathsError {
    inner: imp::JoinPathsError,
}

mod imp {
    use core::fmt;

    /// Named as std's inner error, whose name the `Debug` output shows.
    #[derive(Debug)]
    pub(super) struct JoinPathsError;

    impl fmt::Display for JoinPathsError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(if cfg!(windows) {
                "path segment contains `\"`"
            } else if cfg!(target_family = "wasm") {
                "not supported on this platform yet"
            } else {
                "path segment contains separator `:`"
            })
        }
    }
}

/// Joins a collection of paths appropriately for the `PATH` environment
/// variable.
///
/// # Errors
///
/// Returns an error if a path contains the separator, `:`, on Unix, or a
/// double quote on Windows, where a path containing `;` is quoted instead.
/// On WebAssembly, which has no such convention, it always fails, as in std.
///
/// # Examples
///
/// ```
/// # if cfg!(unix) {
/// let joined = litestd::env::join_paths(["/bin", "/usr/bin"]).unwrap();
/// assert_eq!(joined, "/bin:/usr/bin");
/// assert!(litestd::env::join_paths(["/usr/bi:n"]).is_err());
/// # }
/// ```
pub fn join_paths<I, T>(paths: I) -> Result<OsString, JoinPathsError>
where
    I: IntoIterator<Item = T>,
    T: AsRef<OsStr>,
{
    if cfg!(target_family = "wasm") {
        return Err(JoinPathsError {
            inner: imp::JoinPathsError,
        });
    }
    let mut joined = OsString::new();
    for (i, path) in paths.into_iter().enumerate() {
        if i > 0 {
            joined.push(SEPARATOR);
        }
        push_path(&mut joined, path.as_ref())?;
    }
    Ok(joined)
}

/// Appends `path` to a list being joined.
fn push_path(
    joined: &mut OsString,
    path: &OsStr,
) -> Result<(), JoinPathsError> {
    let bytes = path.as_encoded_bytes();
    let forbidden = if cfg!(windows) { b'"' } else { b':' };
    if bytes.contains(&forbidden) {
        return Err(JoinPathsError {
            inner: imp::JoinPathsError,
        });
    }
    // The separator and quotes are ASCII, so putting them around a path
    // never joins its units with those of another.
    let quote = cfg!(windows) && bytes.contains(&b';');
    if quote {
        joined.push("\"");
    }
    joined.push(path);
    if quote {
        joined.push("\"");
    }
    Ok(())
}

impl fmt::Display for JoinPathsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

impl error::Error for JoinPathsError {}
