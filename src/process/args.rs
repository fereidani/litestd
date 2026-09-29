//! The program and argument storage that every backend shares.

#[cfg(unix)]
use core::ffi::CStr;
use core::{fmt, slice};

use alloc_crate::vec::Vec;

use crate::ffi::OsStr;

/// The program followed by its arguments, stored back to back in one buffer
/// with a NUL byte after each entry, so that a Unix `argv` can point into it
/// and building a command costs two growing allocations.
pub(crate) struct Args {
    /// Every entry followed by a NUL byte, the program first.
    bytes: Vec<u8>,
    /// The offset in `bytes` of each entry's terminating NUL byte.
    ends: Vec<usize>,
    /// Whether an entry holds a NUL byte of its own, which the OS cannot
    /// represent.
    #[cfg(unix)]
    saw_nul: bool,
}

impl Args {
    pub(crate) fn new(program: &OsStr) -> Self {
        let mut args = Self {
            bytes: Vec::new(),
            ends: Vec::new(),
            #[cfg(unix)]
            saw_nul: false,
        };
        args.push(program);
        args
    }

    pub(crate) fn push(&mut self, entry: &OsStr) {
        let entry = entry.as_encoded_bytes();
        #[cfg(unix)]
        {
            self.saw_nul |= entry.contains(&0);
        }
        self.bytes.reserve(entry.len() + 1);
        self.bytes.extend_from_slice(entry);
        self.ends.push(self.bytes.len());
        self.bytes.push(0);
    }

    /// Whether some entry holds a NUL byte, so that spawning must fail.
    #[cfg(unix)]
    pub(crate) const fn saw_nul(&self) -> bool {
        self.saw_nul
    }

    /// The number of entries, the program included.
    #[cfg(any(unix, windows))]
    pub(crate) fn len(&self) -> usize {
        self.ends.len()
    }

    pub(crate) fn program(&self) -> &OsStr {
        self.entries().next().unwrap_or_default()
    }

    /// The arguments after the program.
    pub(crate) fn iter(&self) -> ArgsIter<'_> {
        let mut iter = self.entries();
        iter.next();
        iter
    }

    /// Every entry, the program first.
    pub(crate) fn entries(&self) -> ArgsIter<'_> {
        ArgsIter {
            bytes: &self.bytes,
            start: 0,
            ends: self.ends.iter(),
            #[cfg(windows)]
            index: 0,
            #[cfg(windows)]
            raw: &[],
        }
    }
}

/// An iterator over entries of [`Args`].
#[derive(Clone)]
pub(crate) struct ArgsIter<'a> {
    bytes: &'a [u8],
    /// Where the next entry starts in `bytes`.
    start: usize,
    /// The end offsets of the entries left.
    ends: slice::Iter<'a, usize>,
    /// The position of the next entry, the program being 0.
    #[cfg(windows)]
    index: usize,
    /// The ascending positions of the entries that `raw_arg` added, which
    /// std's `Debug` tells apart.
    #[cfg(windows)]
    raw: &'a [usize],
}

impl<'a> ArgsIter<'a> {
    /// The next entry followed by its NUL byte.
    pub(crate) fn next_with_nul(&mut self) -> Option<&'a [u8]> {
        let end = *self.ends.next()?;
        let entry = self.bytes.get(self.start..=end).unwrap_or_default();
        self.start = end + 1;
        #[cfg(windows)]
        {
            self.index += 1;
        }
        Some(entry)
    }

    /// Marks the entries at the ascending positions `raw` as added by
    /// `raw_arg`, for `Debug`.
    #[cfg(windows)]
    pub(crate) const fn with_raw(mut self, raw: &'a [usize]) -> Self {
        self.raw = raw;
        self
    }
}

impl<'a> Iterator for ArgsIter<'a> {
    type Item = &'a OsStr;

    fn next(&mut self) -> Option<&'a OsStr> {
        let entry = self.next_with_nul()?;
        let entry = entry.split_last().map_or(entry, |(_, entry)| entry);
        // SAFETY: `entry` is exactly the encoded bytes of an `OsStr` that
        // `Args::push` copied, as `ends` records where each copy stops.
        Some(unsafe { OsStr::from_encoded_bytes_unchecked(entry) })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.ends.size_hint()
    }
}

impl ExactSizeIterator for ArgsIter<'_> {
    fn len(&self) -> usize {
        self.ends.len()
    }
}

impl fmt::Debug for ArgsIter<'_> {
    /// Lists the entries as std does: as the C strings it stores on Unix,
    /// tagged `Regular` or `Raw` on Windows, and as OS strings elsewhere.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut list = f.debug_list();
        #[cfg(unix)]
        {
            let mut iter = self.clone();
            while let Some(entry) = iter.next_with_nul() {
                list.entry(&c_str_or_placeholder(entry));
            }
        }
        #[cfg(windows)]
        for (index, arg) in (self.index..).zip(self.clone()) {
            let raw = self.raw.contains(&index);
            list.entry(&Tagged(if raw { "Raw" } else { "Regular" }, arg));
        }
        #[cfg(not(any(unix, windows)))]
        list.entries(self.clone());
        list.finish()
    }
}

/// An argument as std's Windows `Arg` enum shows it.
#[cfg(windows)]
struct Tagged<'a>(&'a str, &'a OsStr);

#[cfg(windows)]
impl fmt::Debug for Tagged<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple(self.0).field(&self.1).finish()
    }
}

/// The C string in `bytes`, which end with a NUL byte, or std's placeholder
/// for a string that holds a NUL byte, as std's Unix backend stores it.
#[cfg(unix)]
pub(crate) fn c_str_or_placeholder(bytes: &[u8]) -> &CStr {
    CStr::from_bytes_with_nul(bytes).unwrap_or(c"<string-with-nul>")
}
