//! Text as `Display` shows it, shared by OS strings and I/O errors: lossy
//! UTF-8, and width and precision honored without `Formatter::pad`, whose
//! character decoding costs more than counting the bytes that start a
//! character.

use core::fmt::{self, Write as _};

/// Writes `bytes` with each invalid UTF-8 sequence replaced by U+FFFD, as
/// `String::from_utf8_lossy` would convert them, without allocating.
#[cfg(feature = "path")]
fn write_lossy(bytes: &[u8], f: &mut fmt::Formatter<'_>) -> fmt::Result {
    for chunk in bytes.utf8_chunks() {
        f.write_str(chunk.valid())?;
        if !chunk.invalid().is_empty() {
            f.write_str("\u{FFFD}")?;
        }
    }
    Ok(())
}

/// Writes `bytes` as [`write_lossy`] does, honoring the width, precision,
/// fill and alignment of `f` as `str` does, each U+FFFD counting as one
/// character.
#[cfg(feature = "path")]
pub(crate) fn fmt_lossy(
    bytes: &[u8],
    f: &mut fmt::Formatter<'_>,
) -> fmt::Result {
    let width = f.width().unwrap_or(0);
    // Without a precision, the truncation only counts the characters.
    let (shown, chars) = match f.precision() {
        None if width == 0 => return write_lossy(bytes, f),
        max => truncate_lossy(bytes, max.unwrap_or(usize::MAX)),
    };
    let (before, after) = padding(f, width, chars);
    let fill = f.fill();
    write_fill(f, fill, before)?;
    write_lossy(shown, f)?;
    write_fill(f, fill, after)
}

/// Writes `s` as `Formatter::pad` does, honoring the width, precision, fill
/// and alignment of `f`.
#[cfg(feature = "io")]
pub(crate) fn pad(s: &str, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let width = f.width().unwrap_or(0);
    let (shown, chars) = match f.precision() {
        None if width == 0 => return f.write_str(s),
        max => truncate(s, max.unwrap_or(usize::MAX)),
    };
    let (before, after) = padding(f, width, chars);
    let fill = f.fill();
    write_fill(f, fill, before)?;
    f.write_str(shown)?;
    write_fill(f, fill, after)
}

/// How many fill characters go before and after text of `chars`
/// characters to make it `width` long: left-aligned by default, as `str`
/// is.
fn padding(
    f: &fmt::Formatter<'_>,
    width: usize,
    chars: usize,
) -> (usize, usize) {
    let padding = width.saturating_sub(chars);
    match f.align() {
        Some(fmt::Alignment::Right) => (padding, 0),
        Some(fmt::Alignment::Center) => (padding / 2, padding - padding / 2),
        Some(fmt::Alignment::Left) | None => (0, padding),
    }
}

/// Writes `count` copies of `fill`.
fn write_fill(
    f: &mut fmt::Formatter<'_>,
    fill: char,
    count: usize,
) -> fmt::Result {
    // Counting down from `count` bounds the loop.
    let mut left = count;
    while left > 0 {
        f.write_char(fill)?;
        left -= 1;
    }
    Ok(())
}

/// Whether `byte` starts a UTF-8 character: all but `0b10xx_xxxx` do.
const fn starts_char(byte: u8) -> bool {
    byte & 0xC0 != 0x80
}

/// Returns the longest prefix of `s` with at most `max` characters, and
/// how many characters that is.
#[cfg(feature = "io")]
fn truncate(s: &str, max: usize) -> (&str, usize) {
    let mut chars = 0;
    for (i, byte) in s.bytes().enumerate() {
        if starts_char(byte) {
            if chars == max {
                return (s.get(..i).unwrap_or(s), max);
            }
            chars += 1;
        }
    }
    (s, chars)
}

/// Returns the longest prefix of `bytes` that [`write_lossy`] writes in at
/// most `max` characters, and how many characters that is.
#[cfg(feature = "path")]
fn truncate_lossy(bytes: &[u8], max: usize) -> (&[u8], usize) {
    let mut left = max;
    let mut offset = 0;
    for chunk in bytes.utf8_chunks() {
        let valid = chunk.valid().bytes().enumerate();
        for (i, _) in valid.filter(|&(_, b)| starts_char(b)) {
            if left == 0 {
                return (bytes.get(..offset + i).unwrap_or(bytes), max);
            }
            left -= 1;
        }
        offset += chunk.valid().len();
        if !chunk.invalid().is_empty() {
            if left == 0 {
                return (bytes.get(..offset).unwrap_or(bytes), max);
            }
            left -= 1;
            offset += chunk.invalid().len();
        }
    }
    (bytes, max - left)
}
