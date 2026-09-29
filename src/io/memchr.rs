//! Byte search for the line-oriented readers and writers: `core` keeps its
//! `memchr` private, so these search a word at a time in safe code.

/// The bytes searched per step.
const WORD: usize = size_of::<usize>();
/// `0x7f` in every byte.
const LOW7: usize = usize::from_ne_bytes([0x7f; WORD]);
/// `0x80` in every byte.
const HIGH: usize = usize::from_ne_bytes([0x80; WORD]);

/// Returns a word whose bytes are `0x80` where `x` has a zero byte and `0`
/// elsewhere. Unlike `(x - 0x01..) & !x & 0x80..`, this has no false
/// positives, so a search from either end can use it.
#[inline]
const fn zero_bytes(x: usize) -> usize {
    !(((x & LOW7).wrapping_add(LOW7)) | x) & HIGH
}

/// Loads a word with the byte at `chunk[0]` in its least significant byte.
#[inline]
fn load(chunk: &[u8]) -> usize {
    usize::from_le_bytes(chunk.try_into().unwrap_or([0; WORD]))
}

/// Returns the index of the first `needle` in `haystack`.
#[inline]
pub(super) fn memchr(needle: u8, haystack: &[u8]) -> Option<usize> {
    let repeated = usize::from_ne_bytes([needle; WORD]);
    let mut chunks = haystack.chunks_exact(WORD);
    let mut offset = 0;
    for chunk in &mut chunks {
        let found = zero_bytes(load(chunk) ^ repeated);
        if found != 0 {
            return Some(offset + found.trailing_zeros() as usize / 8);
        }
        offset += WORD;
    }
    let tail = chunks.remainder().iter().position(|&b| b == needle)?;
    Some(offset + tail)
}

/// Returns the index of the last `needle` in `haystack`.
#[inline]
pub(super) fn memrchr(needle: u8, haystack: &[u8]) -> Option<usize> {
    let repeated = usize::from_ne_bytes([needle; WORD]);
    let mut chunks = haystack.rchunks_exact(WORD);
    let mut end = haystack.len();
    for chunk in &mut chunks {
        let found = zero_bytes(load(chunk) ^ repeated);
        if found != 0 {
            return Some(end - 1 - found.leading_zeros() as usize / 8);
        }
        end -= WORD;
    }
    chunks.remainder().iter().rposition(|&b| b == needle)
}
