//! The Unix encoding of `OsStr`: the bytes themselves, with no constraint.

use core::{
    fmt::{self, Write as _},
    str,
};

use alloc_crate::{borrow::Cow, boxed::Box, string::String, vec::Vec};

use super::{OsStr, write_escaped, write_hex};

/// The storage of an `OsString`.
pub(super) struct Buf {
    pub(super) bytes: Vec<u8>,
}

impl Buf {
    #[inline]
    pub(super) const fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    #[inline]
    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
        }
    }

    #[inline]
    pub(super) fn from_string(s: String) -> Self {
        Self {
            bytes: s.into_bytes(),
        }
    }

    /// # Safety
    ///
    /// None needed on Unix, where any bytes are valid; the function is
    /// `unsafe` to match the Windows encoding.
    #[inline]
    pub(super) const unsafe fn from_encoded_bytes_unchecked(
        bytes: Vec<u8>,
    ) -> Self {
        Self { bytes }
    }

    #[inline]
    pub(super) fn from_os_str(s: &OsStr) -> Self {
        Self {
            bytes: s.inner.to_vec(),
        }
    }

    #[inline]
    pub(super) fn from_box(boxed: Box<OsStr>) -> Self {
        // `into_vec` takes over the allocation of the box.
        Self {
            bytes: boxed.into_boxed_bytes().into_vec(),
        }
    }

    #[inline]
    pub(super) fn push_slice(&mut self, s: &OsStr) {
        self.bytes.extend_from_slice(&s.inner);
    }

    #[inline]
    pub(super) fn push_str(&mut self, s: &str) {
        self.bytes.extend_from_slice(s.as_bytes());
    }

    #[inline]
    pub(super) fn into_string(self) -> Result<String, Self> {
        String::from_utf8(self.bytes).map_err(|error| Self {
            bytes: error.into_bytes(),
        })
    }

    #[inline]
    pub(super) fn clear(&mut self) {
        self.bytes.clear();
    }

    #[inline]
    pub(super) fn truncate(&mut self, len: usize) {
        self.bytes.truncate(len);
    }

    /// Replaces the contents with a copy of `s`, reusing the allocation.
    #[inline]
    pub(super) fn assign(&mut self, s: &OsStr) {
        self.bytes.clear();
        self.bytes.extend_from_slice(&s.inner);
    }
}

impl Clone for Buf {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            bytes: self.bytes.clone(),
        }
    }

    #[inline]
    fn clone_from(&mut self, source: &Self) {
        self.bytes.clone_from(&source.bytes);
    }
}

/// Replaces each invalid UTF-8 sequence with U+FFFD.
#[inline]
pub(super) fn to_string_lossy(bytes: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(bytes)
}

/// Writes `bytes` quoted, with the valid UTF-8 escaped like `str` does and
/// each other byte as `\xHH`.
pub(super) fn fmt_debug(
    bytes: &[u8],
    f: &mut fmt::Formatter<'_>,
) -> fmt::Result {
    f.write_char('"')?;
    for chunk in bytes.utf8_chunks() {
        write_escaped(f, chunk.valid())?;
        for &byte in chunk.invalid() {
            f.write_str("\\x")?;
            write_hex(f, u32::from(byte), 2, true)?;
        }
    }
    f.write_char('"')
}

/// Writes `bytes` with each invalid UTF-8 sequence replaced by U+FFFD,
/// which counts as one character for the width and precision of `f`, as in
/// `str`.
pub(super) use super::fmt_lossy as fmt_display;
