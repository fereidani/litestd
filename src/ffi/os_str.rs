//! The [`OsStr`] and [`OsString`] types and associated utilities.
//!
//! An OS string holds any bytes on Unix and [WTF-8] on Windows, the superset
//! of UTF-8 that can also hold unpaired surrogates, so [`OsStr::to_str`] and
//! [`OsStr::as_encoded_bytes`] borrow on both.
//!
//! [WTF-8]: https://wtf-8.codeberg.page/

// std's methods are not `const`, so that code compiles with either crate.
#![allow(clippy::missing_const_for_fn)]

use core::{
    fmt::{self, Write as _},
    ptr, str,
};

use alloc_crate::{
    borrow::Cow, boxed::Box, collections::TryReserveError, string::String,
    vec::Vec,
};

#[cfg(not(windows))]
mod bytes;
mod impls;
#[cfg(windows)]
pub(crate) mod wtf8;

#[cfg(not(windows))]
use self::bytes as imp;
use self::imp::Buf;
#[cfg(windows)]
use self::wtf8 as imp;
use super::fmt_lossy;

/// An owned, mutable platform string that converts cheaply to and from Rust
/// strings.
///
/// It holds any bytes on Unix and any sequence of 16-bit values on Windows,
/// and is not NUL-terminated. Lengths and capacities count UTF-8 bytes for
/// Unicode contents and units of an unspecified encoding otherwise, the same
/// for every `OsString` and [`OsStr`] on a target.
pub struct OsString {
    inner: Buf,
}

/// A borrowed platform string (akin to `str`); see [`OsString`].
// The casts between `[u8]` and `OsStr` rely on this layout.
#[repr(transparent)]
pub struct OsStr {
    inner: [u8],
}

/// Helper struct for printing an [`OsStr`] with `{}`, replacing non-Unicode
/// data lossily; created by [`OsStr::display`].
pub struct Display<'a> {
    os_str: &'a OsStr,
}

impl OsString {
    /// Constructs a new empty `OsString`.
    #[must_use]
    #[inline]
    pub const fn new() -> Self {
        Self { inner: Buf::new() }
    }

    /// Converts bytes to an `OsString` without checking that the bytes
    /// contain valid [`OsStr`]-encoded data.
    ///
    /// # Safety
    ///
    /// As for [`OsStr::from_encoded_bytes_unchecked`].
    #[allow(clippy::must_use_candidate, reason = "not `#[must_use]` in std")]
    #[inline]
    pub unsafe fn from_encoded_bytes_unchecked(bytes: Vec<u8>) -> Self {
        // SAFETY: bytes that satisfy the contract above are valid in the
        // encoding of this platform, which is all `Buf` requires.
        let inner = unsafe { Buf::from_encoded_bytes_unchecked(bytes) };
        Self { inner }
    }

    /// Converts to an [`OsStr`] slice.
    #[must_use]
    #[inline]
    pub fn as_os_str(&self) -> &OsStr {
        // SAFETY: `Buf` keeps its bytes valid in the encoding of this platform.
        unsafe { OsStr::from_bytes_unchecked(&self.inner.bytes) }
    }

    /// Converts the `OsString` into a byte vector, in the encoding
    /// [`OsStr::as_encoded_bytes`] describes.
    #[allow(clippy::must_use_candidate, reason = "not `#[must_use]` in std")]
    #[inline]
    pub fn into_encoded_bytes(self) -> Vec<u8> {
        self.inner.bytes
    }

    /// Converts the `OsString` into a [`String`] if it is valid Unicode.
    ///
    /// # Errors
    ///
    /// Returns the original `OsString` if it is not valid Unicode.
    #[inline]
    pub fn into_string(self) -> Result<String, Self> {
        self.inner.into_string().map_err(|inner| Self { inner })
    }

    /// Extends the string with the given <code>&[OsStr]</code> slice.
    #[inline]
    pub fn push<T: AsRef<OsStr>>(&mut self, s: T) {
        self.inner.push_slice(s.as_ref());
    }

    /// Creates a new `OsString` with at least the given capacity.
    #[must_use]
    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Buf::with_capacity(capacity),
        }
    }

    /// Truncates the `OsString` to zero length.
    #[inline]
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// Returns the capacity this `OsString` can hold without reallocating.
    #[must_use]
    #[inline]
    pub fn capacity(&self) -> usize {
        self.inner.bytes.capacity()
    }

    /// Reserves capacity for at least `additional` more length units.
    #[inline]
    pub fn reserve(&mut self, additional: usize) {
        self.inner.bytes.reserve(additional);
    }

    /// Tries to reserve capacity for at least `additional` more length units.
    ///
    /// # Errors
    ///
    /// Fails if the capacity overflows or the allocator reports a failure.
    #[inline]
    pub fn try_reserve(
        &mut self,
        additional: usize,
    ) -> Result<(), TryReserveError> {
        self.inner.bytes.try_reserve(additional)
    }

    /// Reserves the minimum capacity for at least `additional` more length
    /// units.
    #[inline]
    pub fn reserve_exact(&mut self, additional: usize) {
        self.inner.bytes.reserve_exact(additional);
    }

    /// Tries to reserve the minimum capacity for at least `additional` more
    /// length units.
    ///
    /// # Errors
    ///
    /// Fails if the capacity overflows or the allocator reports a failure.
    #[inline]
    pub fn try_reserve_exact(
        &mut self,
        additional: usize,
    ) -> Result<(), TryReserveError> {
        self.inner.bytes.try_reserve_exact(additional)
    }

    /// Shrinks the capacity of the `OsString` to match its length.
    #[inline]
    pub fn shrink_to_fit(&mut self) {
        self.inner.bytes.shrink_to_fit();
    }

    /// Shrinks the capacity of the `OsString` with a lower bound.
    #[inline]
    pub fn shrink_to(&mut self, min_capacity: usize) {
        self.inner.bytes.shrink_to(min_capacity);
    }

    /// Converts this `OsString` into a boxed [`OsStr`].
    #[must_use = "`self` will be dropped if the result is not used"]
    pub fn into_boxed_os_str(self) -> Box<OsStr> {
        let bytes = self.inner.bytes.into_boxed_slice();
        // SAFETY: the bytes of an `OsString` are validly encoded.
        unsafe { OsStr::from_boxed_bytes(bytes) }
    }

    /// Consumes and leaks the `OsString`, returning a mutable reference to its
    /// contents, `&'a mut OsStr`, without shrinking the allocation.
    #[allow(clippy::must_use_candidate, reason = "not `#[must_use]` in std")]
    #[inline]
    pub fn leak<'a>(self) -> &'a mut OsStr {
        let bytes = self.inner.bytes.leak();
        // SAFETY: the bytes of an `OsString` are validly encoded.
        unsafe { OsStr::from_bytes_unchecked_mut(bytes) }
    }

    /// Appends a string slice. Unlike [`push`](OsString::push), this needs
    /// no check on Windows: UTF-8 never joins with the end of the string.
    #[inline]
    pub(crate) fn push_str(&mut self, s: &str) {
        self.inner.push_str(s);
    }

    /// Shortens the string to `len` bytes, which must lie at either end or
    /// next to an ASCII byte, as the path code computes it.
    #[inline]
    pub(crate) fn truncate(&mut self, len: usize) {
        self.inner.truncate(len);
    }
}

// Off Windows, for the OS code and the byte-string extensions that pass
// bytes through.
#[cfg(all(
    not(windows),
    any(
        unix,
        all(target_os = "wasi", any(target_env = "p1", feature = "fs")),
        feature = "env"
    )
))]
impl OsString {
    /// Creates an `OsString` from any bytes, which are all valid off Windows.
    #[inline]
    pub(crate) fn from_unix_vec(bytes: Vec<u8>) -> Self {
        Self {
            inner: Buf { bytes },
        }
    }
}

#[cfg(windows)]
impl OsString {
    /// Creates an `OsString` from potentially ill-formed UTF-16, losslessly.
    #[inline]
    pub(crate) fn from_wtf16(wide: &[u16]) -> Self {
        Self {
            inner: Buf::from_wide(wide),
        }
    }
}

impl OsStr {
    /// Coerces into an `OsStr` slice.
    #[inline]
    pub fn new<S: AsRef<Self> + ?Sized>(s: &S) -> &Self {
        s.as_ref()
    }

    /// Converts a slice of bytes to an OS string slice without checking that
    /// the string contains valid `OsStr`-encoded data.
    ///
    /// # Safety
    ///
    /// `bytes` must mix valid UTF-8 with pieces of [`OsStr::as_encoded_bytes`]
    /// from this Rust version and target, split next to non-empty UTF-8.
    #[inline]
    #[allow(clippy::must_use_candidate, reason = "not `#[must_use]` in std")]
    pub unsafe fn from_encoded_bytes_unchecked(bytes: &[u8]) -> &Self {
        // SAFETY: bytes that satisfy the contract above are valid in the
        // encoding of this platform.
        unsafe { Self::from_bytes_unchecked(bytes) }
    }

    /// Wraps bytes in an `OsStr`.
    ///
    /// # Safety
    ///
    /// `bytes` must be valid in the encoding of this platform: well-formed
    /// WTF-8 on Windows; any bytes on Unix.
    #[inline]
    const unsafe fn from_bytes_unchecked(bytes: &[u8]) -> &Self {
        // SAFETY: `OsStr` is a `repr(transparent)` wrapper around `[u8]`, so
        // the cast keeps the address, the length and the layout. The caller
        // guarantees the encoding.
        unsafe { &*(ptr::from_ref(bytes) as *const Self) }
    }

    /// Wraps mutable bytes in an `OsStr`.
    ///
    /// # Safety
    ///
    /// As for [`OsStr::from_bytes_unchecked`].
    #[inline]
    unsafe fn from_bytes_unchecked_mut(bytes: &mut [u8]) -> &mut Self {
        // SAFETY: as in `from_bytes_unchecked`. Every mutation `OsStr` offers
        // only changes the case of ASCII bytes, which keeps the encoding
        // valid.
        unsafe { &mut *(ptr::from_mut(bytes) as *mut Self) }
    }

    /// Wraps a UTF-8 string in an `OsStr`.
    #[inline]
    const fn from_utf8(s: &str) -> &Self {
        // SAFETY: UTF-8 is valid in the encoding of every platform.
        unsafe { Self::from_bytes_unchecked(s.as_bytes()) }
    }

    /// Converts boxed bytes to a boxed `OsStr`.
    ///
    /// # Safety
    ///
    /// As for [`OsStr::from_bytes_unchecked`].
    #[inline]
    unsafe fn from_boxed_bytes(bytes: Box<[u8]>) -> Box<Self> {
        let raw = Box::into_raw(bytes) as *mut Self;
        // SAFETY: `raw` comes from `Box::into_raw` of a `Box<[u8]>`, and
        // `OsStr` is a `repr(transparent)` wrapper around `[u8]`, so the cast
        // keeps the address, the length and the layout of the allocation. The
        // caller guarantees the encoding.
        unsafe { Box::from_raw(raw) }
    }

    /// Converts a boxed `OsStr` to its boxed bytes.
    #[inline]
    fn into_boxed_bytes(self: Box<Self>) -> Box<[u8]> {
        let raw = Box::into_raw(self) as *mut [u8];
        // SAFETY: `raw` comes from `Box::into_raw` of a `Box<OsStr>`, and
        // `OsStr` is a `repr(transparent)` wrapper around `[u8]`, so the cast
        // keeps the address, the length and the layout of the allocation.
        unsafe { Box::from_raw(raw) }
    }

    /// Yields a <code>&[prim@str]</code> slice if it is valid Unicode.
    #[must_use = "this returns the result of the operation, without modifying \
                  the original"]
    #[inline]
    pub fn to_str(&self) -> Option<&str> {
        // WTF-8 is Unicode exactly when it is valid UTF-8.
        str::from_utf8(&self.inner).ok()
    }

    /// Converts an `OsStr` to a <code>[Cow]<[prim@str]></code>, replacing each
    /// invalid UTF-8 sequence on Unix and each unpaired surrogate on Windows
    /// with [`char::REPLACEMENT_CHARACTER`].
    #[must_use = "this returns the result of the operation, without modifying \
                  the original"]
    #[inline]
    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        imp::to_string_lossy(&self.inner)
    }

    /// Copies the slice into an owned [`OsString`].
    #[must_use = "this returns the result of the operation, without modifying \
                  the original"]
    #[inline]
    pub fn to_os_string(&self) -> OsString {
        OsString {
            inner: Buf::from_os_str(self),
        }
    }

    /// Checks whether the `OsStr` is empty.
    #[must_use]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns the length of this `OsStr`; see [`OsString`] for the units.
    #[must_use]
    #[inline]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Converts a <code>[Box]<[OsStr]></code> into an [`OsString`] without
    /// copying or allocating.
    #[must_use = "`self` will be dropped if the result is not used"]
    #[inline]
    pub fn into_os_string(self: Box<Self>) -> OsString {
        OsString {
            inner: Buf::from_box(self),
        }
    }

    /// Converts an OS string slice to a byte slice, in an unspecified,
    /// self-synchronizing superset of UTF-8 whose non-UTF-8 parts are opaque
    /// outside this Rust version and target. To convert back, use
    /// [`OsStr::from_encoded_bytes_unchecked`].
    #[allow(clippy::must_use_candidate, reason = "not `#[must_use]` in std")]
    #[inline]
    pub fn as_encoded_bytes(&self) -> &[u8] {
        &self.inner
    }

    /// Converts this string to its ASCII lower case equivalent in-place.
    #[inline]
    pub fn make_ascii_lowercase(&mut self) {
        self.inner.make_ascii_lowercase();
    }

    /// Converts this string to its ASCII upper case equivalent in-place.
    #[inline]
    pub fn make_ascii_uppercase(&mut self) {
        self.inner.make_ascii_uppercase();
    }

    /// Returns a copy of this string in ASCII lower case.
    #[must_use = "to lowercase the value in-place, use `make_ascii_lowercase`"]
    pub fn to_ascii_lowercase(&self) -> OsString {
        let mut copy = self.to_os_string();
        copy.make_ascii_lowercase();
        copy
    }

    /// Returns a copy of this string in ASCII upper case.
    #[must_use = "to uppercase the value in-place, use `make_ascii_uppercase`"]
    pub fn to_ascii_uppercase(&self) -> OsString {
        let mut copy = self.to_os_string();
        copy.make_ascii_uppercase();
        copy
    }

    /// Checks if all characters in this string are within the ASCII range.
    #[must_use]
    #[inline]
    pub fn is_ascii(&self) -> bool {
        self.inner.is_ascii()
    }

    /// Checks that two strings are an ASCII case-insensitive match.
    #[inline]
    pub fn eq_ignore_ascii_case<S: AsRef<Self>>(&self, other: S) -> bool {
        self.inner.eq_ignore_ascii_case(&other.as_ref().inner)
    }

    /// Returns an object that implements [`Display`](fmt::Display) for
    /// printing an [`OsStr`] that may contain non-Unicode data; use
    /// [`Debug`](fmt::Debug) instead for an escaped form.
    #[must_use = "this does not display the `OsStr`; it returns an object \
                  that can be displayed"]
    #[inline]
    pub fn display(&self) -> Display<'_> {
        Display { os_str: self }
    }
}

#[cfg(all(
    not(windows),
    any(
        unix,
        all(target_os = "wasi", any(target_env = "p1", feature = "fs")),
        feature = "env"
    )
))]
impl OsStr {
    /// Wraps any bytes, which are all valid off Windows.
    #[inline]
    pub(crate) const fn from_unix_bytes(bytes: &[u8]) -> &Self {
        // SAFETY: off Windows, an `OsStr` may hold any bytes.
        unsafe { Self::from_bytes_unchecked(bytes) }
    }
}

/// Writes `s` escaped as `<str as Debug>` does, without the quotes: like
/// [`char::escape_debug`], except that single quotes stay. Printable ASCII,
/// the common case, is flushed in runs.
fn write_escaped(f: &mut fmt::Formatter<'_>, s: &str) -> fmt::Result {
    let mut start = 0;
    for (i, c) in s.char_indices() {
        if matches!(c, ' '..='~') && c != '"' && c != '\\' {
            continue;
        }
        let escape = c.escape_debug();
        if escape.len() != 1 {
            f.write_str(s.get(start..i).unwrap_or_default())?;
            for e in escape {
                f.write_char(e)?;
            }
            start = i + c.len_utf8();
        }
    }
    f.write_str(s.get(start..).unwrap_or_default())
}

/// Writes the low `digits` hexadecimal digits of `value`, at most eight, most
/// significant first, without pulling in integer formatting as `{:X}` would.
pub(crate) fn write_hex(
    f: &mut fmt::Formatter<'_>,
    value: u32,
    digits: usize,
    upper: bool,
) -> fmt::Result {
    // The shift that brings each digit down, most significant first.
    const SHIFTS: [u32; 8] = [28, 24, 20, 16, 12, 8, 4, 0];
    let table: &[u8; 16] = if upper {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    let skip = SHIFTS.len().saturating_sub(digits);
    for &shift in SHIFTS.get(skip..).unwrap_or_default() {
        let nibble = (value >> shift) & 0xF;
        let digit = table.get(nibble as usize).copied().unwrap_or(b'0');
        f.write_char(char::from(digit))?;
    }
    Ok(())
}
