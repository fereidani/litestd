//! Strings stored back to back, each followed by a NUL, the form in which
//! the arguments and the environment are read from the OS.

use core::{fmt, marker::PhantomData, ops::Range};

use alloc_crate::borrow::Cow;

use crate::sys::env::{self, Unit};

/// Strings back to back, each followed by a NUL unit, taken from either end
/// without allocating.
pub(super) struct UnitList {
    buf: Cow<'static, [Unit]>,
    /// Where the first string not yet taken starts.
    front: usize,
    /// Where the last string not yet taken ends, past its NUL.
    back: usize,
    /// The strings not yet taken.
    len: usize,
    /// std's `Args`, `ArgsOs`, `Vars` and `VarsOs` are neither `Send` nor
    /// `Sync`, and neither are the types built on this.
    _not_send: PhantomData<*const ()>,
}

impl UnitList {
    /// Takes over the `len` strings in `buf`, each followed by a NUL.
    pub(super) fn new((buf, len): (Cow<'static, [Unit]>, usize)) -> Self {
        debug_assert!(buf.last().is_none_or(|&unit| unit == 0));
        Self {
            front: 0,
            back: buf.len(),
            buf,
            len,
            _not_send: PhantomData,
        }
    }

    /// The number of strings not yet taken.
    pub(super) const fn len(&self) -> usize {
        self.len
    }

    /// The units at `range`, which a `next` method returned.
    pub(super) fn get(&self, range: Range<usize>) -> &[Unit] {
        self.buf.get(range).unwrap_or_default()
    }

    /// Takes the first string left.
    pub(super) fn next(&mut self) -> Option<&[Unit]> {
        let range = self.next_range()?;
        Some(self.get(range))
    }

    /// Takes the first string left and returns where it is in the units.
    pub(super) fn next_range(&mut self) -> Option<Range<usize>> {
        if self.len == 0 {
            return None;
        }
        let start = self.front;
        let rest = self.buf.get(start..self.back).unwrap_or_default();
        let end = start + env::find_nul(rest).unwrap_or(rest.len());
        self.front = (end + 1).min(self.back);
        self.len -= 1;
        Some(start..end)
    }

    /// Takes the last string left.
    pub(super) fn next_back(&mut self) -> Option<&[Unit]> {
        let range = self.next_back_range()?;
        Some(self.get(range))
    }

    /// Takes the last string left and returns where it is in the units.
    pub(super) fn next_back_range(&mut self) -> Option<Range<usize>> {
        if self.len == 0 {
            return None;
        }
        // The last string's NUL, which ends the strings left.
        let end = self.back.saturating_sub(1);
        let rest = self.buf.get(self.front..end).unwrap_or_default();
        let start = rest
            .iter()
            .rposition(|&unit| unit == 0)
            .map_or(self.front, |nul| self.front + nul + 1);
        self.back = start;
        self.len -= 1;
        Some(start..end)
    }

    /// Drops the next `n` strings, or all those left if fewer.
    pub(super) fn skip(&mut self, n: usize) {
        for _ in 0..n.min(self.len) {
            self.next();
        }
    }

    /// Formats the strings left, each mapped by `map`, as a list.
    pub(super) fn debug<'a, T: fmt::Debug + 'a>(
        &'a self,
        map: fn(&[Unit]) -> T,
    ) -> impl fmt::Debug + 'a {
        Entries { list: self, map }
    }
}

/// Formats the strings left in a list; see [`UnitList::debug`].
struct Entries<'a, T> {
    list: &'a UnitList,
    map: fn(&[Unit]) -> T,
}

impl<T: fmt::Debug> fmt::Debug for Entries<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = self.list;
        let left = list.buf.get(list.front..list.back).unwrap_or_default();
        let strings = left.split(|&unit| unit == 0).take(list.len);
        f.debug_list().entries(strings.map(self.map)).finish()
    }
}
