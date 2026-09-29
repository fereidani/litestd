//! The owned path type, [`PathBuf`].

use alloc_crate::{
    boxed::Box, collections::TryReserveError, string::String, vec::Vec,
};

use super::{
    Component, MAIN_SEPARATOR_STR, Path, borrowed::validate_extension,
    end_offset, is_sep_byte,
};
use crate::ffi::{OsStr, OsString};

/// An owned, mutable path (akin to [`String`]).
///
/// It implements [`Deref`](core::ops::Deref) to [`Path`], so all methods on
/// [`Path`] slices are available on `PathBuf` values too.
///
/// # Examples
///
/// ```
/// use litestd::path::PathBuf;
///
/// let mut path = PathBuf::from("/tmp");
/// path.push("foo");
/// path.set_extension("txt");
/// assert_eq!(path, PathBuf::from("/tmp/foo.txt"));
/// ```
pub struct PathBuf {
    pub(super) inner: OsString,
}

impl PathBuf {
    /// Allocates an empty `PathBuf`.
    #[must_use]
    #[inline]
    pub const fn new() -> Self {
        Self {
            inner: OsString::new(),
        }
    }

    /// Creates a new `PathBuf` with at least the given capacity; see
    /// [`OsString::with_capacity`].
    #[must_use]
    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: OsString::with_capacity(capacity),
        }
    }

    /// Coerces to a [`Path`] slice.
    #[must_use]
    #[inline]
    pub fn as_path(&self) -> &Path {
        Path::new(&self.inner)
    }

    /// Consumes and leaks the `PathBuf`, returning a mutable reference to the
    /// contents, `&'a mut Path`, without shrinking the allocation.
    #[allow(clippy::must_use_candidate, reason = "not `#[must_use]` in std")]
    #[inline]
    pub fn leak<'a>(self) -> &'a mut Path {
        Path::from_os_str_mut(self.inner.leak())
    }

    /// Extends `self` with `path`.
    ///
    /// If `path` is absolute, it replaces the current path. On Windows, a
    /// `path` with a root but no prefix (`\windows`) replaces all but the
    /// prefix of `self`, one with a prefix but no root replaces `self`, and
    /// after a verbatim prefix (`\\?\C:\windows`) a nonempty `path` is added
    /// with its `.` and `..` components resolved.
    pub fn push<P: AsRef<Path>>(&mut self, path: P) {
        self.push_path(path.as_ref());
    }

    fn push_path(&mut self, path: &Path) {
        let comps = self.components();
        let prefix_len = comps.prefix_len();
        // A separator is needed unless the path is empty or already ends
        // with one, or is a drive prefix alone, like `C:`.
        let bytes = self.inner.as_encoded_bytes();
        let need_sep = bytes.last().is_some_and(|&b| !is_sep_byte(b))
            && !(prefix_len == comps.remaining().len()
                && comps.prefix().is_some_and(|p| p.is_drive()));

        let other = path.components();
        // On Unix an absolute `path` replaces `self`; on Windows any `path`
        // with a prefix does, `C:foo` included.
        let replace = if super::HAS_PREFIXES {
            other.prefix().is_some()
        } else {
            other.has_root()
        };
        if replace {
            self.inner.clear();
        } else if comps.prefix_verbatim() && !path.is_empty() {
            self.inner = push_verbatim(comps, path);
            return;
        } else if other.has_root() {
            // A root without a prefix, `\windows`, keeps only the prefix.
            self.inner.truncate(prefix_len);
        } else if need_sep {
            self.inner.reserve(path.as_os_str().len() + 1);
            self.inner.push_str(MAIN_SEPARATOR_STR);
        }
        self.inner.push(path);
    }

    /// Truncates `self` to [`self.parent`](Path::parent), returning `false`
    /// and doing nothing if there is no parent.
    pub fn pop(&mut self) -> bool {
        let Some(len) = self.parent().map(|p| p.as_os_str().len()) else {
            return false;
        };
        self.inner.truncate(len);
        true
    }

    /// Updates [`self.file_name`](Path::file_name) to `file_name`, as
    /// [`pop`](PathBuf::pop) and then [`push`](PathBuf::push) would, or just
    /// the push if there is no file name.
    pub fn set_file_name<S: AsRef<OsStr>>(&mut self, file_name: S) {
        self.set_file_name_os(file_name.as_ref());
    }

    fn set_file_name_os(&mut self, file_name: &OsStr) {
        if self.file_name().is_some() {
            let popped = self.pop();
            debug_assert!(popped);
        }
        self.push(file_name);
    }

    /// Updates [`self.extension`](Path::extension) to `Some(extension)`, or to
    /// `None` if `extension` is empty. Returns `false` and does nothing if
    /// there is no file name.
    ///
    /// # Panics
    ///
    /// Panics if `extension` contains a [path separator](super::is_separator).
    pub fn set_extension<S: AsRef<OsStr>>(&mut self, extension: S) -> bool {
        self.set_extension_os(extension.as_ref())
    }

    fn set_extension_os(&mut self, extension: &OsStr) -> bool {
        validate_extension(extension);
        let Some(stem) = self.file_stem() else {
            return false;
        };
        let end = end_offset(self.inner.as_encoded_bytes(), stem);
        self.inner.truncate(end);
        self.append_extension(extension);
        true
    }

    /// Appends a dot and `extension` to [`self.file_name`](Path::file_name),
    /// unless `extension` is empty. Returns `false` and does nothing if there
    /// is no file name.
    ///
    /// # Panics
    ///
    /// Panics if `extension` contains a [path separator](super::is_separator).
    pub fn add_extension<S: AsRef<OsStr>>(&mut self, extension: S) -> bool {
        self.add_extension_os(extension.as_ref())
    }

    fn add_extension_os(&mut self, extension: &OsStr) -> bool {
        validate_extension(extension);
        let Some(name) = self.file_name() else {
            return false;
        };
        if !extension.is_empty() {
            // Truncate after the file name, dropping trailing separators.
            let end = end_offset(self.inner.as_encoded_bytes(), name);
            self.inner.truncate(end);
            self.append_extension(extension);
        }
        true
    }

    /// Appends a dot and `extension`, unless `extension` is empty.
    fn append_extension(&mut self, extension: &OsStr) {
        if !extension.is_empty() {
            self.inner.reserve_exact(extension.len() + 1);
            self.inner.push_str(".");
            self.inner.push(extension);
        }
    }

    /// Yields a mutable reference to the underlying [`OsString`] instance.
    #[must_use]
    #[inline]
    pub fn as_mut_os_string(&mut self) -> &mut OsString {
        &mut self.inner
    }

    /// Consumes the `PathBuf`, yielding its internal [`OsString`] storage.
    #[must_use = "`self` will be dropped if the result is not used"]
    #[inline]
    pub fn into_os_string(self) -> OsString {
        self.inner
    }

    /// Converts the `PathBuf` into a [`String`] if it is valid Unicode.
    ///
    /// # Errors
    ///
    /// Returns the original `PathBuf` if it is not valid Unicode.
    pub fn into_string(self) -> Result<String, Self> {
        self.inner.into_string().map_err(Self::from)
    }

    /// Converts this `PathBuf` into a [boxed](Box) [`Path`].
    #[must_use = "`self` will be dropped if the result is not used"]
    #[inline]
    pub fn into_boxed_path(self) -> Box<Path> {
        let raw = Box::into_raw(self.inner.into_boxed_os_str()) as *mut Path;
        // SAFETY: `raw` comes from `Box::into_raw` of a `Box<OsStr>`, and
        // `Path` is a `repr(transparent)` wrapper around `OsStr`, so the cast
        // keeps the address, the length and the layout of the allocation.
        unsafe { Box::from_raw(raw) }
    }

    /// Invokes [`OsString::capacity`] on the underlying string.
    #[must_use]
    #[inline]
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    /// Invokes [`OsString::clear`] on the underlying string.
    #[inline]
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// Invokes [`OsString::reserve`] on the underlying string.
    #[inline]
    pub fn reserve(&mut self, additional: usize) {
        self.inner.reserve(additional);
    }

    /// Invokes [`OsString::try_reserve`] on the underlying string.
    ///
    /// # Errors
    ///
    /// Fails if the capacity overflows or the allocator reports a failure.
    #[inline]
    pub fn try_reserve(
        &mut self,
        additional: usize,
    ) -> Result<(), TryReserveError> {
        self.inner.try_reserve(additional)
    }

    /// Invokes [`OsString::reserve_exact`] on the underlying string.
    #[inline]
    pub fn reserve_exact(&mut self, additional: usize) {
        self.inner.reserve_exact(additional);
    }

    /// Invokes [`OsString::try_reserve_exact`] on the underlying string.
    ///
    /// # Errors
    ///
    /// Fails if the capacity overflows or the allocator reports a failure.
    #[inline]
    pub fn try_reserve_exact(
        &mut self,
        additional: usize,
    ) -> Result<(), TryReserveError> {
        self.inner.try_reserve_exact(additional)
    }

    /// Invokes [`OsString::shrink_to_fit`] on the underlying string.
    #[inline]
    pub fn shrink_to_fit(&mut self) {
        self.inner.shrink_to_fit();
    }

    /// Invokes [`OsString::shrink_to`] on the underlying string.
    #[inline]
    pub fn shrink_to(&mut self, min_capacity: usize) {
        self.inner.shrink_to(min_capacity);
    }
}

/// Appends `path` to a path with a verbatim prefix, dropping its `.`
/// components and letting each `..` remove the normal component before it.
fn push_verbatim(comps: super::Components<'_>, path: &Path) -> OsString {
    let mut parts: Vec<Component<'_>> = comps.collect();
    for c in path.components() {
        match c {
            Component::RootDir => {
                parts.truncate(1);
                parts.push(c);
            }
            Component::CurDir => (),
            Component::ParentDir => {
                if let Some(Component::Normal(_)) = parts.last() {
                    parts.pop();
                }
            }
            _ => parts.push(c),
        }
    }
    let mut res = OsString::new();
    let mut need_sep = false;
    for c in parts {
        if need_sep && c != Component::RootDir {
            res.push_str(MAIN_SEPARATOR_STR);
        }
        res.push(c.as_os_str());
        need_sep = match c {
            Component::RootDir => false,
            Component::Prefix(prefix) => !prefix.kind().is_drive(),
            _ => true,
        };
    }
    res
}
