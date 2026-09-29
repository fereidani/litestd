//! Trait implementations for `OsStr`, `OsString` and `Display`.

use core::{
    borrow::Borrow,
    cmp::Ordering,
    convert::Infallible,
    fmt,
    hash::{Hash, Hasher},
    ops,
    str::{self, FromStr, Utf8Error},
};

use alloc_crate::{
    borrow::{Cow, ToOwned},
    boxed::Box,
    rc::Rc,
    string::String,
    sync::Arc,
};

use super::{Buf, Display, OsStr, OsString, imp};

impl From<String> for OsString {
    /// Converts a [`String`] into an [`OsString`] without copying.
    #[inline]
    fn from(s: String) -> Self {
        Self {
            inner: Buf::from_string(s),
        }
    }
}

impl<T: ?Sized + AsRef<OsStr>> From<&T> for OsString {
    /// Copies the string into a new [`OsString`].
    #[inline]
    fn from(s: &T) -> Self {
        s.as_ref().to_os_string()
    }
}

impl ops::Index<ops::RangeFull> for OsString {
    type Output = OsStr;

    #[inline]
    fn index(&self, _index: ops::RangeFull) -> &OsStr {
        self.as_os_str()
    }
}

impl ops::IndexMut<ops::RangeFull> for OsString {
    #[inline]
    fn index_mut(&mut self, _index: ops::RangeFull) -> &mut OsStr {
        // SAFETY: `Buf` keeps its bytes valid in the encoding of this
        // platform.
        unsafe { OsStr::from_bytes_unchecked_mut(&mut self.inner.bytes) }
    }
}

impl ops::Deref for OsString {
    type Target = OsStr;

    #[inline]
    fn deref(&self) -> &OsStr {
        self.as_os_str()
    }
}

impl ops::DerefMut for OsString {
    #[inline]
    fn deref_mut(&mut self) -> &mut OsStr {
        &mut self[..]
    }
}

impl Default for OsString {
    /// Constructs an empty `OsString`.
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for OsString {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }

    /// Clones `source` into `self`, reusing the allocation when possible.
    #[inline]
    fn clone_from(&mut self, source: &Self) {
        self.inner.clone_from(&source.inner);
    }
}

impl fmt::Debug for OsString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_os_str(), f)
    }
}

impl PartialEq for OsString {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.as_os_str() == other.as_os_str()
    }
}

impl Eq for OsString {}

impl PartialOrd for OsString {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialOrd<str> for OsString {
    #[inline]
    fn partial_cmp(&self, other: &str) -> Option<Ordering> {
        self.as_os_str().partial_cmp(other)
    }
}

impl Ord for OsString {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_os_str().cmp(other.as_os_str())
    }
}

impl Hash for OsString {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_os_str().hash(state);
    }
}

impl fmt::Write for OsString {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.push_str(s);
        Ok(())
    }
}

impl From<&OsStr> for Box<OsStr> {
    /// Copies the string into a new boxed [`OsStr`].
    #[inline]
    fn from(s: &OsStr) -> Self {
        let bytes: Box<[u8]> = Box::from(&s.inner);
        // SAFETY: the bytes are copied from an `OsStr`.
        unsafe { OsStr::from_boxed_bytes(bytes) }
    }
}

impl From<&mut OsStr> for Box<OsStr> {
    /// Copies the string into a new boxed [`OsStr`].
    #[inline]
    fn from(s: &mut OsStr) -> Self {
        Self::from(&*s)
    }
}

impl From<Cow<'_, OsStr>> for Box<OsStr> {
    /// Creates a boxed [`OsStr`], copying `cow` only if it is borrowed.
    #[inline]
    fn from(cow: Cow<'_, OsStr>) -> Self {
        match cow {
            Cow::Borrowed(s) => Self::from(s),
            Cow::Owned(s) => Self::from(s),
        }
    }
}

impl From<Box<OsStr>> for OsString {
    /// Converts a boxed [`OsStr`] into an [`OsString`] without copying.
    #[inline]
    fn from(boxed: Box<OsStr>) -> Self {
        boxed.into_os_string()
    }
}

impl From<OsString> for Box<OsStr> {
    /// Converts an [`OsString`] into a boxed [`OsStr`].
    #[inline]
    fn from(s: OsString) -> Self {
        s.into_boxed_os_str()
    }
}

impl Clone for Box<OsStr> {
    #[inline]
    fn clone(&self) -> Self {
        Self::from(&**self)
    }
}

/// Implements the conversions of strings to a reference-counted `OsStr`,
/// which copy the string, for `Arc` and `Rc`.
macro_rules! impl_shared {
    ($($ptr:ident),+) => {$(
        impl From<OsString> for $ptr<OsStr> {
            /// Copies the string into a new reference-counted [`OsStr`].
            #[inline]
            fn from(s: OsString) -> Self {
                Self::from(s.as_os_str())
            }
        }

        impl From<&OsStr> for $ptr<OsStr> {
            /// Copies the string into a new reference-counted [`OsStr`].
            #[inline]
            fn from(s: &OsStr) -> Self {
                let bytes: $ptr<[u8]> = $ptr::from(&s.inner);
                let raw = $ptr::into_raw(bytes) as *const OsStr;
                // SAFETY: `raw` comes from `into_raw` of a pointer to `[u8]`
                // copied from an `OsStr`, and `OsStr` is a `repr(transparent)`
                // wrapper around `[u8]`, so the pointee has the same size,
                // alignment and metadata.
                unsafe { Self::from_raw(raw) }
            }
        }

        impl From<&mut OsStr> for $ptr<OsStr> {
            /// Copies the string into a new reference-counted [`OsStr`].
            #[inline]
            fn from(s: &mut OsStr) -> Self {
                Self::from(&*s)
            }
        }
    )+};
}

impl_shared!(Arc, Rc);

impl From<OsString> for Cow<'_, OsStr> {
    /// Moves the string into a [`Cow::Owned`].
    #[inline]
    fn from(s: OsString) -> Self {
        Cow::Owned(s)
    }
}

impl<'a> From<&'a OsStr> for Cow<'a, OsStr> {
    /// Converts the string reference into a [`Cow::Borrowed`].
    #[inline]
    fn from(s: &'a OsStr) -> Self {
        Cow::Borrowed(s)
    }
}

impl<'a> From<&'a OsString> for Cow<'a, OsStr> {
    /// Converts the string reference into a [`Cow::Borrowed`].
    #[inline]
    fn from(s: &'a OsString) -> Self {
        Cow::Borrowed(s.as_os_str())
    }
}

impl<'a> From<Cow<'a, OsStr>> for OsString {
    /// Converts a `Cow<'a, OsStr>` into an [`OsString`], copying if borrowed.
    #[inline]
    fn from(s: Cow<'a, OsStr>) -> Self {
        s.into_owned()
    }
}

impl<'a> TryFrom<&'a OsStr> for &'a str {
    type Error = Utf8Error;

    /// Tries to convert an `&OsStr` to a `&str`.
    #[inline]
    fn try_from(value: &'a OsStr) -> Result<Self, Self::Error> {
        str::from_utf8(&value.inner)
    }
}

impl Default for Box<OsStr> {
    #[inline]
    fn default() -> Self {
        // SAFETY: the empty string is valid in every encoding.
        unsafe { OsStr::from_boxed_bytes(Box::default()) }
    }
}

impl Default for &OsStr {
    /// Creates an empty `OsStr`.
    #[inline]
    fn default() -> Self {
        OsStr::from_utf8("")
    }
}

impl PartialEq for OsStr {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl PartialEq<str> for OsStr {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        self.inner == *other.as_bytes()
    }
}

impl PartialEq<OsStr> for str {
    #[inline]
    fn eq(&self, other: &OsStr) -> bool {
        *self.as_bytes() == other.inner
    }
}

impl Eq for OsStr {}

impl PartialOrd for OsStr {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialOrd<str> for OsStr {
    #[inline]
    fn partial_cmp(&self, other: &str) -> Option<Ordering> {
        Some(self.inner.cmp(other.as_bytes()))
    }
}

impl Ord for OsStr {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.inner.cmp(&other.inner)
    }
}

/// Implements `PartialEq` both ways between `$lhs`, which dereferences to
/// `OsStr`, and `$rhs`, which dereferences to `$with`, by comparing an `OsStr`
/// with a `$with`.
macro_rules! impl_eq {
    ($lhs:ty, $rhs:ty, $with:ty) => {
        impl PartialEq<$rhs> for $lhs {
            #[inline]
            fn eq(&self, other: &$rhs) -> bool {
                <OsStr as PartialEq<$with>>::eq(self, other)
            }
        }

        impl PartialEq<$lhs> for $rhs {
            #[inline]
            fn eq(&self, other: &$lhs) -> bool {
                <OsStr as PartialEq<$with>>::eq(other, self)
            }
        }
    };
}

/// Implements `PartialEq` and `PartialOrd` both ways between two types that
/// dereference to `OsStr`, by comparing the `OsStr`s.
macro_rules! impl_cmp {
    ($lhs:ty, $rhs:ty) => {
        impl_eq!($lhs, $rhs, OsStr);

        impl PartialOrd<$rhs> for $lhs {
            #[inline]
            fn partial_cmp(&self, other: &$rhs) -> Option<Ordering> {
                <OsStr as PartialOrd>::partial_cmp(self, other)
            }
        }

        impl PartialOrd<$lhs> for $rhs {
            #[inline]
            fn partial_cmp(&self, other: &$lhs) -> Option<Ordering> {
                <OsStr as PartialOrd>::partial_cmp(self, other)
            }
        }
    };
}

impl_cmp!(OsString, OsStr);
impl_cmp!(OsString, &OsStr);
impl_cmp!(Cow<'_, OsStr>, OsStr);
impl_cmp!(Cow<'_, OsStr>, &OsStr);
impl_cmp!(Cow<'_, OsStr>, OsString);
impl_eq!(OsString, str, str);
impl_eq!(OsString, &str, str);

impl Hash for OsStr {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.inner.hash(state);
    }
}

impl fmt::Debug for OsStr {
    /// Formats the string in double quotes, escaped like `str`. Bytes that
    /// are not UTF-8 appear as `\xHH` on Unix, and unpaired surrogates as
    /// `\u{dxxx}` on Windows.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        imp::fmt_debug(&self.inner, f)
    }
}

impl fmt::Debug for Display<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.os_str, f)
    }
}

impl fmt::Display for Display<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        imp::fmt_display(&self.os_str.inner, f)
    }
}

impl Borrow<OsStr> for OsString {
    #[inline]
    fn borrow(&self) -> &OsStr {
        self.as_os_str()
    }
}

impl ToOwned for OsStr {
    type Owned = OsString;

    #[inline]
    fn to_owned(&self) -> OsString {
        self.to_os_string()
    }

    #[inline]
    fn clone_into(&self, target: &mut OsString) {
        target.inner.assign(self);
    }
}

impl AsRef<Self> for OsStr {
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

impl AsRef<OsStr> for OsString {
    #[inline]
    fn as_ref(&self) -> &OsStr {
        self.as_os_str()
    }
}

impl AsRef<OsStr> for str {
    #[inline]
    fn as_ref(&self) -> &OsStr {
        OsStr::from_utf8(self)
    }
}

impl AsRef<OsStr> for String {
    #[inline]
    fn as_ref(&self) -> &OsStr {
        // SAFETY: a `String` holds UTF-8, which is valid in the encoding of
        // every platform.
        unsafe { OsStr::from_bytes_unchecked(self.as_bytes()) }
    }
}

impl FromStr for OsString {
    type Err = Infallible;

    #[inline]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::from(String::from(s)))
    }
}

impl Extend<Self> for OsString {
    #[inline]
    fn extend<I: IntoIterator<Item = Self>>(&mut self, iter: I) {
        for s in iter {
            self.push(&s);
        }
    }
}

impl<'a> Extend<&'a OsStr> for OsString {
    #[inline]
    fn extend<I: IntoIterator<Item = &'a OsStr>>(&mut self, iter: I) {
        for s in iter {
            self.push(s);
        }
    }
}

impl<'a> Extend<Cow<'a, OsStr>> for OsString {
    #[inline]
    fn extend<I: IntoIterator<Item = Cow<'a, OsStr>>>(&mut self, iter: I) {
        for s in iter {
            self.push(&s);
        }
    }
}

impl FromIterator<Self> for OsString {
    #[inline]
    fn from_iter<I: IntoIterator<Item = Self>>(iter: I) -> Self {
        let mut iter = iter.into_iter();
        // Appending to the first string saves an allocation.
        let mut buf = iter.next().unwrap_or_default();
        buf.extend(iter);
        buf
    }
}

impl<'a> FromIterator<&'a OsStr> for OsString {
    #[inline]
    fn from_iter<I: IntoIterator<Item = &'a OsStr>>(iter: I) -> Self {
        let mut buf = Self::new();
        buf.extend(iter);
        buf
    }
}

impl<'a> FromIterator<Cow<'a, OsStr>> for OsString {
    #[inline]
    fn from_iter<I: IntoIterator<Item = Cow<'a, OsStr>>>(iter: I) -> Self {
        let mut iter = iter.into_iter();
        // Appending to the first owned string saves an allocation.
        let mut buf = iter.next().map(Cow::into_owned).unwrap_or_default();
        buf.extend(iter);
        buf
    }
}
