//! Windows path prefixes: `C:`, `\\server\share` and their verbatim forms.

use core::{
    cmp::Ordering,
    hash::{Hash, Hasher},
};

#[cfg(windows)]
use super::{is_sep, is_sep_byte, os_str_from_piece};
use crate::ffi::OsStr;

/// Windows path prefixes, e.g., `C:` or `\\server\share`.
///
/// After a verbatim prefix (`\\?\`), `/` is not a separator and essentially no
/// normalization is performed.
#[derive(Copy, Clone, Debug, Hash, PartialOrd, Ord, PartialEq, Eq)]
#[allow(clippy::upper_case_acronyms, reason = "the names std uses")]
pub enum Prefix<'a> {
    /// Verbatim prefix, e.g., `\\?\cat_pics`.
    Verbatim(&'a OsStr),

    /// Verbatim UNC prefix, e.g., `\\?\UNC\server\share`.
    VerbatimUNC(&'a OsStr, &'a OsStr),

    /// Verbatim disk prefix, e.g., `\\?\C:`.
    VerbatimDisk(u8),

    /// Device namespace prefix, e.g., `\\.\COM42`, where `/` may replace `\`.
    DeviceNS(&'a OsStr),

    /// Prefix using Windows' Uniform Naming Convention, e.g. `\\server\share`.
    UNC(&'a OsStr, &'a OsStr),

    /// Prefix `C:` for the given disk drive.
    Disk(u8),
}

impl Prefix<'_> {
    /// The length of the prefix in the path it was parsed from.
    #[inline]
    pub(super) fn len(&self) -> usize {
        // The share is optional in the verbatim form, and so is the
        // separator before it.
        fn share_len(share: &OsStr) -> usize {
            if share.is_empty() { 0 } else { 1 + share.len() }
        }
        match *self {
            Self::Verbatim(name) | Self::DeviceNS(name) => 4 + name.len(),
            Self::VerbatimUNC(server, share) => {
                8 + server.len() + share_len(share)
            }
            Self::VerbatimDisk(_) => 6,
            Self::UNC(server, share) => 2 + server.len() + share_len(share),
            Self::Disk(_) => 2,
        }
    }

    /// Determines if the prefix is verbatim, i.e., begins with `\\?\`.
    #[inline]
    #[must_use]
    pub fn is_verbatim(&self) -> bool {
        matches!(
            *self,
            Self::Verbatim(_) | Self::VerbatimDisk(_) | Self::VerbatimUNC(..)
        )
    }

    /// Whether this is a plain drive prefix, `C:`.
    #[inline]
    pub(super) const fn is_drive(&self) -> bool {
        matches!(*self, Self::Disk(_))
    }

    /// Whether the prefix roots the path by itself: all but `C:` do.
    #[inline]
    pub(super) const fn has_implicit_root(&self) -> bool {
        !self.is_drive()
    }
}

/// A Windows path prefix, parsed and raw, from a
/// [`Component::Prefix`](super::Component::Prefix); it does not occur on Unix.
#[derive(Copy, Clone, Eq, Debug)]
pub struct PrefixComponent<'a> {
    pub(super) raw: &'a OsStr,
    pub(super) parsed: Prefix<'a>,
}

impl<'a> PrefixComponent<'a> {
    /// Returns the parsed prefix data.
    #[must_use]
    #[inline]
    pub fn kind(&self) -> Prefix<'a> {
        self.parsed
    }

    /// Returns the raw [`OsStr`] slice for this prefix.
    #[must_use]
    #[inline]
    pub fn as_os_str(&self) -> &'a OsStr {
        self.raw
    }
}

impl PartialEq for PrefixComponent<'_> {
    /// Compares the parsed prefixes: `c:` and `C:` are equal.
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.parsed == other.parsed
    }
}

impl PartialOrd for PrefixComponent<'_> {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PrefixComponent<'_> {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.parsed.cmp(&other.parsed)
    }
}

impl Hash for PrefixComponent<'_> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.parsed.hash(h);
    }
}

/// Parses the prefix of a Windows path, if it has one.
///
/// The first eight bytes decide the kind, with `/` read as `\`, except that
/// `\\?\` must be spelled with backslashes.
#[cfg(windows)]
pub(super) fn parse_prefix(path: &OsStr) -> Option<Prefix<'_>> {
    let bytes = path.as_encoded_bytes();
    let rest = match *bytes {
        [first, second, ref rest @ ..]
            if is_sep_byte(first) && is_sep_byte(second) =>
        {
            rest
        }
        _ => return parse_disk(bytes).map(Prefix::Disk),
    };
    if let [b'\\', b'\\', b'?', b'\\', ref after @ ..] = *bytes {
        return Some(parse_verbatim(after));
    }
    if let [b'.', sep, ref device @ ..] = *rest {
        if is_sep_byte(sep) {
            return Some(Prefix::DeviceNS(split_component(device, false).0));
        }
    }
    let (server, rest) = split_component(rest, false);
    let (share, _) = split_component(rest, false);
    if server.is_empty() || share.is_empty() {
        return None;
    }
    Some(Prefix::UNC(server, share))
}

/// Parses the prefix of a path: Unix paths have none.
#[cfg(not(windows))]
#[inline]
pub(super) const fn parse_prefix(_path: &OsStr) -> Option<Prefix<'_>> {
    None
}

/// Parses what follows `\\?\`.
#[cfg(windows)]
fn parse_verbatim(after: &[u8]) -> Prefix<'_> {
    if let [b'U', b'N', b'C', sep, ref rest @ ..] = *after {
        if is_sep_byte(sep) {
            let (server, rest) = split_component(rest, true);
            let (share, _) = split_component(rest, true);
            return Prefix::VerbatimUNC(server, share);
        }
    }
    // Only an exact drive, `C:` alone or before a backslash, is a disk.
    let exact = after.get(2).is_none_or(|&b| is_sep(b, true));
    match parse_disk(after) {
        Some(drive) if exact => Prefix::VerbatimDisk(drive),
        _ => Prefix::Verbatim(split_component(after, true).0),
    }
}

/// Parses a drive letter and colon, returning the letter in upper case.
#[cfg(windows)]
fn parse_disk(bytes: &[u8]) -> Option<u8> {
    match *bytes {
        [drive, b':', ..] if drive.is_ascii_alphabetic() => {
            Some(drive.to_ascii_uppercase())
        }
        _ => None,
    }
}

/// Splits `bytes` at its first separator, returning the part before it and
/// the part after it, or all of `bytes` and an empty rest.
#[cfg(windows)]
fn split_component(bytes: &[u8], verbatim: bool) -> (&OsStr, &[u8]) {
    let mut parts = bytes.splitn(2, |&b| is_sep(b, verbatim));
    let component = parts.next().unwrap_or_default();
    let rest = parts.next().unwrap_or_default();
    // SAFETY: `component` lies between the start of the path, or a
    // separator the prefix syntax requires, and a separator or the end.
    (unsafe { os_str_from_piece(component) }, rest)
}
