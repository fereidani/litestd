//! The byte conversions of [`OsStr`] and [`OsString`], shared by the Unix
//! and WASI modules, where strings are bytes.

use alloc_crate::vec::Vec;

use crate::ffi::{OsStr, OsString};

mod private {
    /// Seals the extension traits, as in std, so litestd may add methods.
    pub trait Sealed {}

    impl Sealed for crate::ffi::OsString {}
    impl Sealed for crate::ffi::OsStr {}
}

/// Platform-specific extensions to [`OsString`].
///
/// This trait is sealed: it cannot be implemented outside litestd.
pub trait OsStringExt: private::Sealed {
    /// Creates an [`OsString`] from a byte vector.
    fn from_vec(vec: Vec<u8>) -> Self;

    /// Yields the underlying byte vector of this [`OsString`].
    fn into_vec(self) -> Vec<u8>;
}

impl OsStringExt for OsString {
    #[inline]
    fn from_vec(vec: Vec<u8>) -> Self {
        Self::from_unix_vec(vec)
    }

    #[inline]
    fn into_vec(self) -> Vec<u8> {
        self.into_encoded_bytes()
    }
}

/// Platform-specific extensions to [`OsStr`].
///
/// This trait is sealed: it cannot be implemented outside litestd.
pub trait OsStrExt: private::Sealed {
    /// Creates an [`OsStr`] from a byte slice.
    fn from_bytes(slice: &[u8]) -> &Self;

    /// Gets the underlying byte view of the [`OsStr`] slice.
    fn as_bytes(&self) -> &[u8];
}

impl OsStrExt for OsStr {
    #[inline]
    fn from_bytes(slice: &[u8]) -> &Self {
        Self::from_unix_bytes(slice)
    }

    #[inline]
    fn as_bytes(&self) -> &[u8] {
        self.as_encoded_bytes()
    }
}
