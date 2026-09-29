//! The borrowed path type, [`Path`].

use core::{error, fmt, ptr};

use alloc_crate::{borrow::Cow, boxed::Box};

use super::{
    PathBuf,
    components::{Ancestors, Component, Components, Iter, iter_after},
    end_offset, is_sep_byte, os_str_from_piece, path_from_piece,
};
use crate::ffi::{OsStr, OsString, os_str};

/// A slice of a path (akin to [`str`]).
///
/// This is an unsized type, used behind a pointer like `&` or [`Box`]; the
/// owned form is [`PathBuf`].
///
/// # Examples
///
/// ```
/// use litestd::{ffi::OsStr, path::Path};
///
/// let path = Path::new("./foo/bar.txt");
/// assert_eq!(path.parent(), Some(Path::new("./foo")));
/// assert_eq!(path.extension(), Some(OsStr::new("txt")));
/// ```
// The casts between `OsStr` and `Path` rely on this layout.
#[repr(transparent)]
pub struct Path {
    inner: OsStr,
}

/// The error [`Path::strip_prefix`] returns if the prefix was not found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripPrefixError(());

/// Helper struct for printing paths with `{}`, created by [`Path::display`].
///
/// Non-Unicode data is replaced lossily, depending on the platform; use
/// [`Debug`](fmt::Debug) instead for an escaped form.
pub struct Display<'a> {
    inner: os_str::Display<'a>,
}

impl Path {
    /// Directly wraps a string slice as a `Path` slice, at no cost.
    #[inline]
    pub fn new<S: AsRef<OsStr> + ?Sized>(s: &S) -> &Self {
        Self::from_os_str(s.as_ref())
    }

    #[inline]
    const fn from_os_str(s: &OsStr) -> &Self {
        // SAFETY: `Path` is a `repr(transparent)` wrapper around `OsStr`, so
        // the cast keeps the address, the length and the layout.
        unsafe { &*(ptr::from_ref(s) as *const Self) }
    }

    #[inline]
    pub(super) fn from_os_str_mut(s: &mut OsStr) -> &mut Self {
        // SAFETY: as in `from_os_str`.
        unsafe { &mut *(ptr::from_mut(s) as *mut Self) }
    }

    /// Yields the underlying [`OsStr`] slice.
    #[must_use]
    #[inline]
    pub fn as_os_str(&self) -> &OsStr {
        &self.inner
    }

    /// Yields a mutable reference to the underlying [`OsStr`] slice.
    #[must_use]
    #[inline]
    pub fn as_mut_os_str(&mut self) -> &mut OsStr {
        &mut self.inner
    }

    /// Yields a [`&str`](str) slice if the `Path` is valid Unicode.
    #[must_use = "this returns the result of the operation, without modifying \
                  the original"]
    #[inline]
    pub fn to_str(&self) -> Option<&str> {
        self.inner.to_str()
    }

    /// Converts a `Path` to a <code>[Cow]<[str]></code>, replacing any
    /// non-UTF-8 sequences with [`char::REPLACEMENT_CHARACTER`].
    #[must_use = "this returns the result of the operation, without modifying \
                  the original"]
    #[inline]
    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        self.inner.to_string_lossy()
    }

    /// Converts a `Path` to an owned [`PathBuf`].
    #[must_use = "this returns the result of the operation, without modifying \
                  the original"]
    pub fn to_path_buf(&self) -> PathBuf {
        PathBuf::from(self.inner.to_os_string())
    }

    /// Returns `true` if the `Path` is absolute, i.e., independent of the
    /// current directory: on Unix if it has a root, on Windows if it also has
    /// a prefix, so `c:\windows` is absolute but `c:temp` and `\temp` are not.
    #[must_use]
    pub fn is_absolute(&self) -> bool {
        let comps = self.components();
        // As in std, a root alone suffices only on Unix and WASI; without a
        // file system, as on wasm32-unknown-unknown, no path is absolute.
        comps.has_root()
            && (cfg!(any(unix, target_os = "wasi")) || comps.prefix().is_some())
    }

    /// Returns `true` if the `Path` is relative, i.e., not absolute.
    #[must_use]
    #[inline]
    pub fn is_relative(&self) -> bool {
        !self.is_absolute()
    }

    /// Returns `true` if the `Path` has a root.
    ///
    /// On Unix that is a leading `/`. On Windows it is a separator that starts
    /// the path or follows a prefix (`\windows`, `c:\windows` but not
    /// `c:windows`), or any non-disk prefix (`\\server\share`).
    #[must_use]
    #[inline]
    pub fn has_root(&self) -> bool {
        self.components().has_root()
    }

    /// Returns the `Path` without its final component, if there is one.
    ///
    /// Returns `Some("")` for a relative path with one component, and [`None`]
    /// if the path ends in a root or prefix, or is empty.
    #[doc(alias = "dirname")]
    #[must_use]
    pub fn parent(&self) -> Option<&Self> {
        let mut comps = self.components();
        match comps.next_back()? {
            Component::Normal(_) | Component::CurDir | Component::ParentDir => {
                Some(comps.as_path())
            }
            Component::Prefix(_) | Component::RootDir => None,
        }
    }

    /// Produces an iterator over `Path` and its ancestors: `self`, then each
    /// [`parent`](Path::parent) until there is none.
    #[inline]
    pub fn ancestors(&self) -> Ancestors<'_> {
        Ancestors { next: Some(self) }
    }

    /// Returns the final component of the `Path`, the name of a file or
    /// directory, if there is one; [`None`] if the path ends in `..`.
    #[doc(alias = "basename")]
    #[must_use]
    pub fn file_name(&self) -> Option<&OsStr> {
        match self.components().next_back()? {
            Component::Normal(name) => Some(name),
            _ => None,
        }
    }

    /// Returns a path that, when joined onto `base`, yields `self`.
    ///
    /// # Errors
    ///
    /// Fails if `base` is not a prefix of `self`, as
    /// [`starts_with`](Path::starts_with) tests.
    pub fn strip_prefix<P>(&self, base: P) -> Result<&Self, StripPrefixError>
    where
        P: AsRef<Self>,
    {
        self.strip_prefix_path(base.as_ref())
    }

    fn strip_prefix_path(
        &self,
        base: &Self,
    ) -> Result<&Self, StripPrefixError> {
        iter_after(self.components(), base.components())
            .map(|c| c.as_path())
            .ok_or(StripPrefixError(()))
    }

    /// Determines whether `base` is a prefix of `self`, matching whole
    /// components only.
    #[must_use]
    pub fn starts_with<P: AsRef<Self>>(&self, base: P) -> bool {
        self.starts_with_path(base.as_ref())
    }

    fn starts_with_path(&self, base: &Self) -> bool {
        iter_after(self.components(), base.components()).is_some()
    }

    /// Determines whether `child` is a suffix of `self`, matching whole
    /// components only.
    #[must_use]
    pub fn ends_with<P: AsRef<Self>>(&self, child: P) -> bool {
        self.ends_with_path(child.as_ref())
    }

    fn ends_with_path(&self, child: &Self) -> bool {
        iter_after(self.components().rev(), child.components().rev()).is_some()
    }

    /// Checks whether the `Path` is empty.
    #[allow(clippy::must_use_candidate, reason = "not `#[must_use]` in std")]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Extracts the stem of [`self.file_name`](Path::file_name): the part
    /// before its final `.`, or all of it if it has no `.` past its start.
    #[must_use]
    pub fn file_stem(&self) -> Option<&OsStr> {
        let (before, after) = rsplit_file_at_dot(self.file_name()?);
        before.or(after)
    }

    /// Extracts the prefix of [`self.file_name`](Path::file_name): the part
    /// before its first `.` past its start, or all of it if there is none.
    #[must_use]
    pub fn file_prefix(&self) -> Option<&OsStr> {
        self.file_name().map(|name| split_file_at_dot(name).0)
    }

    /// Extracts the extension of [`self.file_name`](Path::file_name), without
    /// the dot: the part after its final `.`, if that `.` is past its start.
    #[must_use]
    pub fn extension(&self) -> Option<&OsStr> {
        let (before, after) = rsplit_file_at_dot(self.file_name()?);
        before.and(after)
    }

    /// Creates an owned [`PathBuf`] with `path` adjoined to `self`, as
    /// [`PathBuf::push`] does.
    #[must_use]
    pub fn join<P: AsRef<Self>>(&self, path: P) -> PathBuf {
        self.join_path(path.as_ref())
    }

    fn join_path(&self, path: &Self) -> PathBuf {
        // One allocation holds both parts and a separator.
        let capacity = self.inner.len() + path.inner.len() + 1;
        let mut buf = PathBuf::with_capacity(capacity);
        buf.as_mut_os_string().push(&self.inner);
        buf.push(path);
        buf
    }

    /// Creates an owned [`PathBuf`] like `self` but with the given file name;
    /// see [`PathBuf::set_file_name`].
    #[must_use]
    pub fn with_file_name<S: AsRef<OsStr>>(&self, file_name: S) -> PathBuf {
        self.with_file_name_os(file_name.as_ref())
    }

    fn with_file_name_os(&self, file_name: &OsStr) -> PathBuf {
        let mut buf = self.to_path_buf();
        buf.set_file_name(file_name);
        buf
    }

    /// Creates an owned [`PathBuf`] like `self` but with the given extension;
    /// see [`PathBuf::set_extension`].
    ///
    /// # Panics
    ///
    /// Panics if `extension` contains a [path separator](super::is_separator).
    pub fn with_extension<S: AsRef<OsStr>>(&self, extension: S) -> PathBuf {
        self.with_extension_os(extension.as_ref())
    }

    fn with_extension_os(&self, extension: &OsStr) -> PathBuf {
        validate_extension(extension);
        let bytes = self.inner.as_encoded_bytes();
        // std cuts the old extension off the end of the path before setting
        // the new one, the same as setting it on the whole path except for a
        // path ending in `..ext`: `..` is left, which gets no extension.
        if let Some(old) = self.extension() {
            if end_offset(bytes, old) == bytes.len()
                && self.file_stem().is_some_and(|stem| stem == ".")
            {
                let head = bytes.get(..bytes.len() - old.len());
                // SAFETY: `head` ends right after the dot before `old`.
                let head = unsafe { path_from_piece(head.unwrap_or(bytes)) };
                return head.to_path_buf();
            }
        }
        // One allocation holds the path, a dot and the new extension.
        let capacity = bytes.len() + extension.len() + 1;
        let mut buf = PathBuf::with_capacity(capacity);
        buf.as_mut_os_string().push(&self.inner);
        buf.set_extension(extension);
        buf
    }

    /// Creates an owned [`PathBuf`] like `self` but with the extension added;
    /// see [`PathBuf::add_extension`]. A path without a file name is returned
    /// unchanged.
    ///
    /// # Panics
    ///
    /// Panics if `extension` contains a [path separator](super::is_separator).
    pub fn with_added_extension<S: AsRef<OsStr>>(
        &self,
        extension: S,
    ) -> PathBuf {
        self.with_added_extension_os(extension.as_ref())
    }

    fn with_added_extension_os(&self, extension: &OsStr) -> PathBuf {
        let capacity = self.inner.len() + extension.len() + 1;
        let mut buf = PathBuf::with_capacity(capacity);
        buf.as_mut_os_string().push(&self.inner);
        buf.add_extension(extension);
        buf
    }

    /// Produces an iterator over the [`Component`]s of the path.
    ///
    /// Repeated separators, non-leading `.` components and trailing separators
    /// are skipped: `a//b`, `a/./b` and `a/b/` all yield `a` and `b`, while
    /// `./a` starts with [`CurDir`](Component::CurDir). `a/b/../c` keeps its
    /// `..`, since `b` may be a symbolic link.
    #[inline]
    pub fn components(&self) -> Components<'_> {
        Components::new(self)
    }

    /// Produces an iterator over the path's components viewed as [`OsStr`]
    /// slices; see [`components`](Path::components).
    #[inline]
    pub fn iter(&self) -> Iter<'_> {
        Iter {
            inner: self.components(),
        }
    }

    /// Returns an object that implements [`Display`](fmt::Display) for
    /// printing paths that may contain non-Unicode data.
    #[must_use = "this does not display the path, it returns an object that \
                  can be displayed"]
    #[inline]
    pub fn display(&self) -> Display<'_> {
        Display {
            inner: self.inner.display(),
        }
    }

    /// Converts a [`Box<Path>`](Box) into a [`PathBuf`] without copying or
    /// allocating.
    #[must_use = "`self` will be dropped if the result is not used"]
    pub fn into_path_buf(self: Box<Self>) -> PathBuf {
        let raw = Box::into_raw(self) as *mut OsStr;
        // SAFETY: `raw` comes from `Box::into_raw` of a `Box<Path>`, and `Path`
        // is a `repr(transparent)` wrapper around `OsStr`, so the cast keeps
        // the address, the length and the layout of the allocation.
        let inner = unsafe { Box::from_raw(raw) };
        PathBuf::from(OsString::from(inner))
    }
}

impl fmt::Debug for Display<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.inner, f)
    }
}

impl fmt::Display for Display<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.inner, f)
    }
}

impl fmt::Display for StripPrefixError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("prefix not found")
    }
}

impl error::Error for StripPrefixError {}

/// Splits a file name at its last dot. `..` and a name whose only dot starts
/// it are all before the dot; a name without a dot is all after it.
fn rsplit_file_at_dot(file: &OsStr) -> (Option<&OsStr>, Option<&OsStr>) {
    let bytes = file.as_encoded_bytes();
    if bytes == b".." {
        return (Some(file), None);
    }
    let mut parts = bytes.rsplitn(2, |&b| b == b'.');
    let after = parts.next();
    let before = parts.next();
    if before == Some(b"") {
        return (Some(file), None);
    }
    // SAFETY: both parts lie between a dot and an end of the file name.
    unsafe {
        (
            before.map(|s| os_str_from_piece(s)),
            after.map(|s| os_str_from_piece(s)),
        )
    }
}

/// Splits a file name at its first dot that does not start it.
fn split_file_at_dot(file: &OsStr) -> (&OsStr, Option<&OsStr>) {
    let bytes = file.as_encoded_bytes();
    if bytes == b".." {
        return (file, None);
    }
    let Some(dot) = bytes.iter().skip(1).position(|&b| b == b'.') else {
        return (file, None);
    };
    // `dot` counts from the second byte.
    let (Some(before), Some(after)) = (bytes.get(..=dot), bytes.get(dot + 2..))
    else {
        return (file, None);
    };
    // SAFETY: both parts lie between the dot and an end of the file name.
    unsafe { (os_str_from_piece(before), Some(os_str_from_piece(after))) }
}

/// Panics if `extension` contains a path separator, as the std contract of
/// `set_extension` and the other extension setters states.
#[inline]
#[track_caller]
pub(super) fn validate_extension(extension: &OsStr) {
    if extension.as_encoded_bytes().iter().any(|&b| is_sep_byte(b)) {
        extension_has_separator();
    }
}

#[cold]
#[inline(never)]
#[track_caller]
#[allow(clippy::panic)]
fn extension_has_separator() -> ! {
    panic!("extension cannot contain path separators")
}
