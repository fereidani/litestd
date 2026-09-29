//! Windows-specific extensions to [`OsStr`] and [`OsString`].
//!
//! Windows strings may be ill-formed UTF-16 with unpaired surrogates.
//! [`OsStringExt::from_wide`] and [`OsStrExt::encode_wide`] convert them
//! without loss; [`OsStr::to_string_lossy`] replaces each unpaired surrogate
//! with U+FFFD.

use core::{fmt, iter::FusedIterator, mem, slice};

use crate::ffi::{OsStr, OsString, os_str::wtf8};

mod private {
    /// Seals the extension traits so that, as in std, methods can be added.
    pub trait Sealed {}

    impl Sealed for crate::ffi::OsString {}
    impl Sealed for crate::ffi::OsStr {}
}

/// Windows-specific extensions to [`OsString`]. This trait is sealed: it
/// cannot be implemented outside litestd.
pub trait OsStringExt: private::Sealed {
    /// Creates an `OsString` from a potentially ill-formed UTF-16 slice of
    /// 16-bit code units, without loss.
    fn from_wide(wide: &[u16]) -> Self;
}

impl OsStringExt for OsString {
    #[inline]
    fn from_wide(wide: &[u16]) -> Self {
        Self::from_wtf16(wide)
    }
}

/// Windows-specific extensions to [`OsStr`]. This trait is sealed: it
/// cannot be implemented outside litestd.
pub trait OsStrExt: private::Sealed {
    /// Re-encodes an `OsStr` as potentially ill-formed UTF-16, without loss and
    /// without a final NUL terminator.
    fn encode_wide(&self) -> EncodeWide<'_>;
}

impl OsStrExt for OsStr {
    #[inline]
    fn encode_wide(&self) -> EncodeWide<'_> {
        EncodeWide {
            bytes: self.as_encoded_bytes().iter(),
            low: 0,
        }
    }
}

/// Iterator returned by [`OsStrExt::encode_wide`].
#[derive(Clone)]
pub struct EncodeWide<'a> {
    /// The WTF-8 left to encode.
    bytes: slice::Iter<'a, u8>,
    /// The low surrogate still to yield after its high one, or 0.
    low: u16,
}

impl Iterator for EncodeWide<'_> {
    type Item = u16;

    #[inline]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "a code point below U+10000, or 10 bits of one above, fits \
                  in a code unit"
    )]
    fn next(&mut self) -> Option<u16> {
        if self.low != 0 {
            return Some(mem::take(&mut self.low));
        }
        let code_point = wtf8::next_code_point(&mut self.bytes)?;
        let Some(supplementary) = code_point.checked_sub(0x1_0000) else {
            // Below U+10000, surrogates included: one code unit.
            return Some(code_point as u16);
        };
        self.low = 0xDC00 | (supplementary & 0x3FF) as u16;
        Some(0xD800 | (supplementary >> 10) as u16)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        // A code point takes one to four bytes and yields one or two units.
        let bytes = self.bytes.len();
        let pending = usize::from(self.low != 0);
        let low = bytes.saturating_add(3) / 4 + pending;
        let high = bytes.checked_mul(2).and_then(|n| n.checked_add(pending));
        (low, high)
    }
}

impl FusedIterator for EncodeWide<'_> {}

impl fmt::Debug for EncodeWide<'_> {
    /// Lists the code units left, as `char`s where valid and in hex otherwise.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct CodeUnit(u16);

        impl fmt::Debug for CodeUnit {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let unit = u32::from(self.0);
                if let Some(c) = char::from_u32(unit) {
                    return fmt::Debug::fmt(&c, f);
                }
                f.write_str("0x")?;
                crate::ffi::os_str::write_hex(f, unit, 4, true)
            }
        }

        f.write_str("EncodeWide(")?;
        f.debug_list()
            .entries(self.clone().map(CodeUnit))
            .finish()?;
        f.write_str(")")
    }
}
