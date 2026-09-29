//! Trait implementations for `Path` and `PathBuf`.

use core::{
    borrow::Borrow,
    cmp::Ordering,
    convert::Infallible,
    fmt,
    hash::{Hash, Hasher},
    ops,
    str::FromStr,
};

use alloc_crate::{
    borrow::{Cow, ToOwned},
    boxed::Box,
    rc::Rc,
    string::String,
    sync::Arc,
};

use super::{
    Iter, Path, PathBuf, components::compare_components, is_sep,
    prefix::parse_prefix,
};
use crate::ffi::{OsStr, OsString};

impl Clone for PathBuf {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            inner: self.as_os_str().to_os_string(),
        }
    }

    /// Clones `source` into `self`, reusing the allocation when possible.
    #[inline]
    fn clone_from(&mut self, source: &Self) {
        self.inner.clone_from(&source.inner);
    }
}

impl From<&Path> for Box<Path> {
    /// Creates a boxed [`Path`] by copying `path`.
    fn from(path: &Path) -> Self {
        // The copy has no spare capacity, so boxing it does not reallocate.
        path.to_path_buf().into_boxed_path()
    }
}

impl From<&mut Path> for Box<Path> {
    /// Creates a boxed [`Path`] by copying `path`.
    fn from(path: &mut Path) -> Self {
        Self::from(&*path)
    }
}

impl From<Cow<'_, Path>> for Box<Path> {
    /// Creates a boxed [`Path`], copying `cow` only if it is borrowed.
    #[inline]
    fn from(cow: Cow<'_, Path>) -> Self {
        match cow {
            Cow::Borrowed(path) => Self::from(path),
            Cow::Owned(path) => Self::from(path),
        }
    }
}

impl From<Box<Path>> for PathBuf {
    /// Converts a boxed [`Path`] into a [`PathBuf`] without copying.
    #[inline]
    fn from(boxed: Box<Path>) -> Self {
        boxed.into_path_buf()
    }
}

impl From<PathBuf> for Box<Path> {
    /// Converts a [`PathBuf`] into a boxed [`Path`].
    #[inline]
    fn from(p: PathBuf) -> Self {
        p.into_boxed_path()
    }
}

impl Clone for Box<Path> {
    #[inline]
    fn clone(&self) -> Self {
        Self::from(&**self)
    }
}

impl<T: ?Sized + AsRef<OsStr>> From<&T> for PathBuf {
    /// Converts a borrowed [`OsStr`] to a [`PathBuf`] by copying it.
    #[inline]
    fn from(s: &T) -> Self {
        Self {
            inner: s.as_ref().to_os_string(),
        }
    }
}

impl From<OsString> for PathBuf {
    /// Converts an [`OsString`] into a [`PathBuf`] without copying.
    #[inline]
    fn from(inner: OsString) -> Self {
        Self { inner }
    }
}

impl From<PathBuf> for OsString {
    /// Converts a [`PathBuf`] into an [`OsString`] without copying.
    #[inline]
    fn from(path_buf: PathBuf) -> Self {
        path_buf.into_os_string()
    }
}

impl From<String> for PathBuf {
    /// Converts a [`String`] into a [`PathBuf`] without copying.
    #[inline]
    fn from(s: String) -> Self {
        Self::from(OsString::from(s))
    }
}

impl FromStr for PathBuf {
    type Err = Infallible;

    #[inline]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::from(String::from(s)))
    }
}

impl<P: AsRef<Path>> FromIterator<P> for PathBuf {
    /// Creates a `PathBuf`, [pushing](Self::push) each path of the iterator.
    fn from_iter<I: IntoIterator<Item = P>>(iter: I) -> Self {
        let mut buf = Self::new();
        buf.extend(iter);
        buf
    }
}

impl<P: AsRef<Path>> Extend<P> for PathBuf {
    /// Extends `self`, [pushing](Self::push) each path of the iterator.
    fn extend<I: IntoIterator<Item = P>>(&mut self, iter: I) {
        for p in iter {
            self.push(p.as_ref());
        }
    }
}

impl fmt::Debug for PathBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_path(), f)
    }
}

impl ops::Deref for PathBuf {
    type Target = Path;

    #[inline]
    fn deref(&self) -> &Path {
        self.as_path()
    }
}

impl ops::DerefMut for PathBuf {
    #[inline]
    fn deref_mut(&mut self) -> &mut Path {
        Path::from_os_str_mut(&mut self.inner)
    }
}

impl Borrow<Path> for PathBuf {
    #[inline]
    fn borrow(&self) -> &Path {
        self.as_path()
    }
}

impl Default for PathBuf {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> From<&'a Path> for Cow<'a, Path> {
    /// Creates a [`Cow::Borrowed`] from a reference to [`Path`].
    #[inline]
    fn from(s: &'a Path) -> Self {
        Cow::Borrowed(s)
    }
}

impl From<PathBuf> for Cow<'_, Path> {
    /// Creates a [`Cow::Owned`] from an owned [`PathBuf`].
    #[inline]
    fn from(s: PathBuf) -> Self {
        Cow::Owned(s)
    }
}

impl<'a> From<&'a PathBuf> for Cow<'a, Path> {
    /// Creates a [`Cow::Borrowed`] from a reference to [`PathBuf`].
    #[inline]
    fn from(p: &'a PathBuf) -> Self {
        Cow::Borrowed(p.as_path())
    }
}

impl<'a> From<Cow<'a, Path>> for PathBuf {
    /// Converts a clone-on-write pointer to an owned path, copying if borrowed.
    #[inline]
    fn from(p: Cow<'a, Path>) -> Self {
        p.into_owned()
    }
}

/// Implements the conversions of paths to a reference-counted `Path`, which
/// copy the path, for `Arc` and `Rc`.
macro_rules! impl_shared {
    ($($ptr:ident),+) => {$(
        impl From<PathBuf> for $ptr<Path> {
            /// Copies the path into a new reference-counted [`Path`].
            #[inline]
            fn from(s: PathBuf) -> Self {
                Self::from(s.as_path())
            }
        }

        impl From<&Path> for $ptr<Path> {
            /// Copies the path into a new reference-counted [`Path`].
            #[inline]
            fn from(s: &Path) -> Self {
                let raw = $ptr::into_raw($ptr::<OsStr>::from(s.as_os_str()));
                // SAFETY: `raw` comes from `into_raw` of a pointer to an
                // `OsStr`, and `Path` is a `repr(transparent)` wrapper around
                // `OsStr`, so the pointee has the same size, alignment and
                // metadata.
                unsafe { Self::from_raw(raw as *const Path) }
            }
        }

        impl From<&mut Path> for $ptr<Path> {
            /// Copies the path into a new reference-counted [`Path`].
            #[inline]
            fn from(s: &mut Path) -> Self {
                Self::from(&*s)
            }
        }
    )+};
}

impl_shared!(Arc, Rc);

impl ToOwned for Path {
    type Owned = PathBuf;

    #[inline]
    fn to_owned(&self) -> PathBuf {
        self.to_path_buf()
    }

    #[inline]
    fn clone_into(&self, target: &mut PathBuf) {
        self.as_os_str().clone_into(target.as_mut_os_string());
    }
}

impl PartialEq for PathBuf {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.components() == other.components()
    }
}

impl Hash for PathBuf {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.as_path().hash(h);
    }
}

impl Eq for PathBuf {}

impl PartialOrd for PathBuf {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PathBuf {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        compare_components(self.components(), other.components())
    }
}

impl AsRef<OsStr> for PathBuf {
    #[inline]
    fn as_ref(&self) -> &OsStr {
        self.as_os_str()
    }
}

impl fmt::Debug for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_os_str(), f)
    }
}

impl PartialEq for Path {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.components() == other.components()
    }
}

impl Hash for Path {
    /// Hashes the prefix, then the bytes of each component, skipping what
    /// [`Path::components`] skips, then a value derived from their lengths.
    ///
    /// Unlike std, this also skips a `.` after a Windows drive prefix, as in
    /// `C:.`, so that equal paths such as `C:.` and `C:` hash the same.
    fn hash<H: Hasher>(&self, h: &mut H) {
        let mut bytes = self.as_os_str().as_encoded_bytes();
        let prefix = parse_prefix(self.as_os_str());
        let verbatim = prefix.is_some_and(|p| p.is_verbatim());
        if let Some(prefix) = prefix {
            prefix.hash(h);
            bytes = bytes.get(prefix.len()..).unwrap_or_default();
        }
        // Outside verbatim paths, `components` drops a `.` after a
        // separator, and after a prefix, but keeps a leading one.
        let mut dot_is_dropped = prefix.is_some() && !verbatim;
        let mut chunk_bits: usize = 0;
        for part in bytes.split(|&b| is_sep(b, verbatim)) {
            let dropped = part.is_empty() || (dot_is_dropped && part == b".");
            dot_is_dropped = !verbatim;
            if dropped {
                continue;
            }
            chunk_bits = chunk_bits.wrapping_add(part.len()).rotate_right(2);
            h.write(part);
        }
        h.write_usize(chunk_bits);
    }
}

impl Eq for Path {}

impl PartialOrd for Path {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Path {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        compare_components(self.components(), other.components())
    }
}

impl AsRef<OsStr> for Path {
    #[inline]
    fn as_ref(&self) -> &OsStr {
        self.as_os_str()
    }
}

impl AsRef<Self> for Path {
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

/// Implements `AsRef<Path>` for the string types, which `Path::new` wraps.
macro_rules! impl_as_path {
    ($($ty:ty),+) => {$(
        impl AsRef<Path> for $ty {
            #[inline]
            fn as_ref(&self) -> &Path {
                Path::new(self)
            }
        }
    )+};
}

impl_as_path!(OsStr, Cow<'_, OsStr>, OsString, str, String);

impl AsRef<Path> for PathBuf {
    #[inline]
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl<'a> IntoIterator for &'a PathBuf {
    type Item = &'a OsStr;
    type IntoIter = Iter<'a>;

    #[inline]
    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

impl<'a> IntoIterator for &'a Path {
    type Item = &'a OsStr;
    type IntoIter = Iter<'a>;

    #[inline]
    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

/// Views a value that converts to a `Path` as one, for `impl_eq` and
/// `impl_cmp`.
#[inline]
fn as_path<P: AsRef<Path> + ?Sized>(path: &P) -> &Path {
    path.as_ref()
}

/// Implements `PartialEq` both ways between two types that convert to `Path`,
/// by comparing the `Path`s.
macro_rules! impl_eq {
    ($lhs:ty, $rhs:ty) => {
        impl PartialEq<$rhs> for $lhs {
            #[inline]
            fn eq(&self, other: &$rhs) -> bool {
                as_path(self) == as_path(other)
            }
        }

        impl PartialEq<$lhs> for $rhs {
            #[inline]
            fn eq(&self, other: &$lhs) -> bool {
                as_path(self) == as_path(other)
            }
        }
    };
}

/// Implements `PartialEq` and `PartialOrd` both ways between two types that
/// convert to `Path`, by comparing the `Path`s.
macro_rules! impl_cmp {
    ($lhs:ty, $rhs:ty) => {
        impl_eq!($lhs, $rhs);

        impl PartialOrd<$rhs> for $lhs {
            #[inline]
            fn partial_cmp(&self, other: &$rhs) -> Option<Ordering> {
                as_path(self).partial_cmp(as_path(other))
            }
        }

        impl PartialOrd<$lhs> for $rhs {
            #[inline]
            fn partial_cmp(&self, other: &$lhs) -> Option<Ordering> {
                as_path(self).partial_cmp(as_path(other))
            }
        }
    };
}

impl_cmp!(PathBuf, Path);
impl_cmp!(PathBuf, &Path);
impl_cmp!(Cow<'_, Path>, Path);
impl_cmp!(Cow<'_, Path>, &Path);
impl_cmp!(Cow<'_, Path>, PathBuf);
impl_cmp!(PathBuf, OsStr);
impl_cmp!(PathBuf, &OsStr);
impl_cmp!(PathBuf, Cow<'_, OsStr>);
impl_cmp!(PathBuf, OsString);
impl_cmp!(Path, OsStr);
impl_cmp!(Path, &OsStr);
impl_cmp!(Path, Cow<'_, OsStr>);
impl_cmp!(Path, OsString);
impl_cmp!(&Path, OsStr);
impl_cmp!(&Path, Cow<'_, OsStr>);
impl_cmp!(&Path, OsString);
impl_cmp!(Cow<'_, Path>, OsStr);
impl_cmp!(Cow<'_, Path>, &OsStr);
impl_cmp!(Cow<'_, Path>, OsString);
impl_eq!(PathBuf, str);
impl_eq!(PathBuf, String);
impl_eq!(Path, str);
impl_eq!(Path, String);
