//! The Windows encoding of `OsStr`: [WTF-8].
//!
//! WTF-8 extends UTF-8 with the surrogates U+D800 to U+DFFF, encoded in three
//! bytes as `ED A0..=BF 80..=BF`. It is well-formed when no leading surrogate
//! (`ED A0..=AF xx`) directly precedes a trailing one (`ED B0..=BF xx`): such
//! a pair is stored as the supplementary code point it encodes, as
//! concatenating the UTF-16 would. Every `OsStr` holds well-formed WTF-8, so
//! one without surrogates is UTF-8. Slicing next to an ASCII byte keeps it
//! well-formed, and so does concatenation that pairs the surrogates at the
//! seam, as [`Buf::push_slice`] does.
//!
//! [WTF-8]: https://wtf-8.codeberg.page/

use core::{
    fmt::{self, Write as _},
    slice, str,
};

use alloc_crate::{borrow::Cow, boxed::Box, string::String, vec::Vec};

use super::{OsStr, write_escaped, write_hex};

/// The storage of an `OsString`: well-formed WTF-8 in `bytes`, which is also
/// UTF-8 whenever `known_utf8` is `true`.
pub(super) struct Buf {
    pub(super) bytes: Vec<u8>,
    /// Lets `into_string` skip validation. Set by constructors that know their
    /// contents are UTF-8, and cleared when a surrogate may appear.
    known_utf8: bool,
}

impl Buf {
    #[inline]
    pub(super) const fn new() -> Self {
        Self {
            bytes: Vec::new(),
            known_utf8: true,
        }
    }

    #[inline]
    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
            known_utf8: true,
        }
    }

    #[inline]
    pub(super) fn from_string(s: String) -> Self {
        Self {
            bytes: s.into_bytes(),
            known_utf8: true,
        }
    }

    /// # Safety
    ///
    /// `bytes` must be well-formed WTF-8.
    #[inline]
    pub(super) const unsafe fn from_encoded_bytes_unchecked(
        bytes: Vec<u8>,
    ) -> Self {
        Self {
            bytes,
            known_utf8: false,
        }
    }

    #[inline]
    pub(super) fn from_os_str(s: &OsStr) -> Self {
        Self {
            bytes: s.inner.to_vec(),
            known_utf8: false,
        }
    }

    #[inline]
    pub(super) fn from_box(boxed: Box<OsStr>) -> Self {
        // `into_vec` takes over the allocation of the box.
        let bytes = boxed.into_boxed_bytes().into_vec();
        Self {
            bytes,
            known_utf8: false,
        }
    }

    /// Decodes potentially ill-formed UTF-16 losslessly.
    pub(super) fn from_wide(wide: &[u16]) -> Self {
        let mut buf = Self::with_capacity(wide.len());
        // Paths are mostly ASCII: narrow the leading ASCII run in one pass
        // that the compiler vectorizes.
        let ascii = wide.iter().position(|&unit| unit >= 0x80);
        let split = wide.split_at_checked(ascii.unwrap_or(wide.len()));
        let (head, rest) = split.unwrap_or((wide, &[]));
        #[allow(
            clippy::cast_possible_truncation,
            reason = "every unit of the run is below 0x80"
        )]
        buf.bytes.extend(head.iter().map(|&unit| unit as u8));
        let mut utf8 = [0; 4];
        for unit in char::decode_utf16(rest.iter().copied()) {
            match unit {
                Ok(c) => {
                    let encoded = c.encode_utf8(&mut utf8);
                    buf.bytes.extend_from_slice(encoded.as_bytes());
                }
                Err(error) => {
                    // `decode_utf16` has already paired every surrogate it
                    // could, so this one cannot pair with its neighbours
                    // and the result stays well-formed.
                    buf.known_utf8 = false;
                    let surrogate = error.unpaired_surrogate();
                    buf.bytes.extend_from_slice(&encode_surrogate(surrogate));
                }
            }
        }
        buf
    }

    /// Appends well-formed WTF-8, pairing a leading surrogate at the end of
    /// the string with a trailing surrogate at the start of `s`, as
    /// concatenating the UTF-16 forms would.
    pub(super) fn push_slice(&mut self, s: &OsStr) {
        let other = &s.inner;
        let lead = final_lead_surrogate(&self.bytes);
        let Some((lead, (trail, rest))) =
            lead.zip(initial_trail_surrogate(other))
        else {
            // A string that is known to be UTF-8 stays so unless `other`
            // brings a surrogate.
            if self.known_utf8 && has_surrogate(other) {
                self.known_utf8 = false;
            }
            self.bytes.extend_from_slice(other);
            return;
        };
        // The string ended with a surrogate, so `known_utf8` is already
        // `false`. Replace both halves with the four-byte code point.
        self.bytes.truncate(self.bytes.len().saturating_sub(3));
        self.bytes.reserve(4 + rest.len());
        self.bytes.extend_from_slice(&encode_pair(lead, trail));
        self.bytes.extend_from_slice(rest);
    }

    /// Appends UTF-8. A `str` holds no surrogate, so nothing can pair and
    /// `known_utf8` stays accurate.
    #[inline]
    pub(super) fn push_str(&mut self, s: &str) {
        self.bytes.extend_from_slice(s.as_bytes());
    }

    pub(super) fn into_string(self) -> Result<String, Self> {
        if self.known_utf8 {
            // SAFETY: `known_utf8` is only set while the bytes are UTF-8.
            return Ok(unsafe { String::from_utf8_unchecked(self.bytes) });
        }
        String::from_utf8(self.bytes).map_err(|error| Self {
            bytes: error.into_bytes(),
            known_utf8: false,
        })
    }

    #[inline]
    pub(super) fn clear(&mut self) {
        self.bytes.clear();
        self.known_utf8 = true;
    }

    /// Shortens the string to `len` bytes, moving a cut inside a code point
    /// back to its start, so that the string stays well-formed WTF-8, and
    /// UTF-8 if it was. Callers only cut next to ASCII bytes.
    pub(super) fn truncate(&mut self, len: usize) {
        let mut len = len;
        // A code point spans at most four bytes, three after its first, so
        // the cut moves back at most three times.
        let mut steps = 3;
        while steps > 0 {
            match self.bytes.get(len) {
                Some(&byte) if is_continuation(byte) => {
                    len = len.saturating_sub(1);
                }
                _ => break,
            }
            steps -= 1;
        }
        debug_assert!(self.bytes.get(len).is_none_or(|&b| !is_continuation(b)));
        self.bytes.truncate(len);
    }

    /// Replaces the contents with a copy of `s`, reusing the allocation.
    #[inline]
    pub(super) fn assign(&mut self, s: &OsStr) {
        self.bytes.clear();
        self.bytes.extend_from_slice(&s.inner);
        self.known_utf8 = false;
    }
}

impl Clone for Buf {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            bytes: self.bytes.clone(),
            known_utf8: self.known_utf8,
        }
    }

    #[inline]
    fn clone_from(&mut self, source: &Self) {
        self.bytes.clone_from(&source.bytes);
        self.known_utf8 = source.known_utf8;
    }
}

/// Whether `byte` continues a code point rather than starting one.
#[inline]
const fn is_continuation(byte: u8) -> bool {
    byte & 0xC0 == 0x80
}

/// Decodes the surrogate encoded as `ED b2 b3`.
#[inline]
fn decode_surrogate(b2: u8, b3: u8) -> u16 {
    0xD800 | (u16::from(b2 & 0x3F) << 6) | u16::from(b3 & 0x3F)
}

/// Encodes a surrogate code point in three bytes.
#[inline]
#[allow(
    clippy::cast_possible_truncation,
    reason = "each operand is masked to six bits"
)]
const fn encode_surrogate(surrogate: u16) -> [u8; 3] {
    let high = ((surrogate >> 6) & 0x3F) as u8;
    let low = (surrogate & 0x3F) as u8;
    [0xED, 0x80 | high, 0x80 | low]
}

/// Encodes the supplementary code point of a surrogate pair in four bytes.
#[inline]
#[allow(
    clippy::cast_possible_truncation,
    reason = "each operand is masked to fit its byte"
)]
fn encode_pair(lead: u16, trail: u16) -> [u8; 4] {
    let high = u32::from(lead & 0x3FF);
    let low = u32::from(trail & 0x3FF);
    let code_point = 0x10000 + ((high << 10) | low);
    [
        0xF0 | (code_point >> 18) as u8,
        0x80 | ((code_point >> 12) & 0x3F) as u8,
        0x80 | ((code_point >> 6) & 0x3F) as u8,
        0x80 | (code_point & 0x3F) as u8,
    ]
}

/// Returns the leading surrogate that ends `bytes`, if there is one.
#[inline]
fn final_lead_surrogate(bytes: &[u8]) -> Option<u16> {
    match *bytes {
        [.., 0xED, b2 @ 0xA0..=0xAF, b3] => Some(decode_surrogate(b2, b3)),
        _ => None,
    }
}

/// Returns the trailing surrogate that starts `bytes`, if any, and the rest.
#[inline]
fn initial_trail_surrogate(bytes: &[u8]) -> Option<(u16, &[u8])> {
    match *bytes {
        [0xED, b2 @ 0xB0..=0xBF, b3, ref rest @ ..] => {
            Some((decode_surrogate(b2, b3), rest))
        }
        _ => None,
    }
}

/// Whether well-formed WTF-8 holds a surrogate: `0xED` is always a lead byte,
/// and starts a surrogate exactly when the next byte is at least `0xA0`. Most
/// strings have no `0xED`, which `contains` finds a word at a time.
fn has_surrogate(bytes: &[u8]) -> bool {
    bytes.contains(&0xED)
        && bytes
            .windows(2)
            .any(|pair| matches!(pair, [0xED, 0xA0..=0xFF]))
}

/// Decodes the next code point of well-formed WTF-8, surrogates included. On
/// malformed input the result is meaningless, but memory-safe and panic-free.
#[inline]
pub(crate) fn next_code_point(bytes: &mut slice::Iter<'_, u8>) -> Option<u32> {
    let first = *bytes.next()?;
    if first < 0x80 {
        return Some(u32::from(first));
    }
    let mut continuation =
        || bytes.next().map_or(0, |&byte| u32::from(byte & 0x3F));
    let second = continuation();
    if first < 0xE0 {
        return Some((u32::from(first & 0x1F) << 6) | second);
    }
    let third = continuation();
    let low = (second << 6) | third;
    if first < 0xF0 {
        return Some((u32::from(first & 0x0F) << 12) | low);
    }
    let fourth = continuation();
    Some((u32::from(first & 0x07) << 18) | (low << 6) | fourth)
}

/// Splits well-formed WTF-8 into UTF-8 runs, each followed by the surrogate
/// that ends it, if any.
struct Pieces<'a> {
    rest: Option<&'a [u8]>,
}

impl<'a> Pieces<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { rest: Some(bytes) }
    }
}

impl<'a> Iterator for Pieces<'a> {
    type Item = (&'a str, Option<u16>);

    fn next(&mut self) -> Option<Self::Item> {
        let rest = self.rest?;
        match str::from_utf8(rest) {
            Ok(valid) => {
                self.rest = None;
                Some((valid, None))
            }
            Err(error) => {
                // In well-formed WTF-8, validation only stops at a surrogate.
                // Validating the run again is cheap and keeps this code safe.
                let split = rest.split_at_checked(error.valid_up_to());
                let (valid, tail) = split.unwrap_or((&[], rest));
                let valid = str::from_utf8(valid).unwrap_or_default();
                if let [0xED, b2, b3, ref after @ ..] = *tail {
                    self.rest = Some(after);
                    Some((valid, Some(decode_surrogate(b2, b3))))
                } else {
                    self.rest = None;
                    Some((valid, None))
                }
            }
        }
    }
}

/// Replaces each surrogate with U+FFFD, borrowing when there is none.
pub(super) fn to_string_lossy(bytes: &[u8]) -> Cow<'_, str> {
    if let Ok(s) = str::from_utf8(bytes) {
        return Cow::Borrowed(s);
    }
    // U+FFFD takes three bytes, like the surrogate it replaces.
    let mut lossy = String::with_capacity(bytes.len());
    for (valid, surrogate) in Pieces::new(bytes) {
        lossy.push_str(valid);
        if surrogate.is_some() {
            lossy.push(char::REPLACEMENT_CHARACTER);
        }
    }
    Cow::Owned(lossy)
}

/// Writes `bytes` quoted, with the UTF-8 escaped like `str` does and each
/// surrogate as `\u{dxxx}`.
pub(super) fn fmt_debug(
    bytes: &[u8],
    f: &mut fmt::Formatter<'_>,
) -> fmt::Result {
    f.write_char('"')?;
    for (valid, surrogate) in Pieces::new(bytes) {
        write_escaped(f, valid)?;
        if let Some(surrogate) = surrogate {
            f.write_str("\\u{")?;
            write_hex(f, u32::from(surrogate), 4, false)?;
            f.write_char('}')?;
        }
    }
    f.write_char('"')
}

/// Writes `bytes` with each surrogate replaced by U+FFFD. Like std, only a
/// string without surrogates honors padding, as a `str` does.
pub(super) fn fmt_display(
    bytes: &[u8],
    f: &mut fmt::Formatter<'_>,
) -> fmt::Result {
    if str::from_utf8(bytes).is_ok() {
        return super::fmt_lossy(bytes, f);
    }
    for (valid, surrogate) in Pieces::new(bytes) {
        f.write_str(valid)?;
        if surrogate.is_some() {
            f.write_char(char::REPLACEMENT_CHARACTER)?;
        }
    }
    Ok(())
}
