//! Snapshots of the environment variables.

use core::fmt;

use alloc_crate::{borrow::Cow, string::String};

use super::list::UnitList;
use crate::{
    ffi::OsString,
    sys::{self, env::Unit},
};

/// An iterator over a snapshot of the environment variables of this
/// process.
///
/// This structure is created by [`vars`].
pub struct Vars {
    inner: VarsOs,
}

/// An iterator over a snapshot of the environment variables of this
/// process.
///
/// This structure is created by [`vars_os`].
pub struct VarsOs {
    list: UnitList,
}

/// Returns an iterator of (variable, value) pairs of strings, for all the
/// environment variables of the current process.
///
/// The iterator holds a snapshot taken now; later changes to the
/// environment do not show in it. Each pair is copied out as it is yielded.
///
/// # Panics
///
/// While iterating, the returned iterator panics if any key or value in the
/// environment is not valid Unicode. Use [`vars_os`] to avoid that.
///
/// # Examples
///
/// ```
/// for (key, value) in litestd::env::vars() {
///     assert!(!key.is_empty() && !value.contains('\0'));
/// }
/// ```
#[must_use]
pub fn vars() -> Vars {
    Vars { inner: vars_os() }
}

/// Returns an iterator of (variable, value) pairs of OS strings, for all the
/// environment variables of the current process.
///
/// As [`vars`], without checking that the pairs are Unicode.
#[must_use]
pub fn vars_os() -> VarsOs {
    let (buf, len) = sys::env::vars();
    VarsOs {
        list: UnitList::new((Cow::Owned(buf), len)),
    }
}

impl Iterator for Vars {
    type Item = (String, String);

    fn next(&mut self) -> Option<(String, String)> {
        let (key, value) = self.inner.next()?;
        Some((into_string(key), into_string(value)))
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl fmt::Debug for Vars {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Vars")
            .field("inner", &self.inner.list.debug(split))
            .finish()
    }
}

impl Iterator for VarsOs {
    type Item = (OsString, OsString);

    fn next(&mut self) -> Option<(OsString, OsString)> {
        self.list.next().map(split)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.list.len(), Some(self.list.len()))
    }

    #[inline]
    fn count(self) -> usize {
        self.list.len()
    }
}

impl fmt::Debug for VarsOs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VarsOs")
            .field("inner", &self.list.debug(split))
            .finish()
    }
}

/// Splits an entry at the first `=` after its first unit: names are never
/// empty, and may start with `=` on Windows.
fn split(entry: &[Unit]) -> (OsString, OsString) {
    let equals = Unit::from(b'=');
    let at = entry
        .get(1..)
        .and_then(|rest| rest.iter().position(|&unit| unit == equals))
        .map_or(entry.len(), |i| i + 1);
    let (key, value) = entry.split_at_checked(at).unwrap_or((entry, &[]));
    let value = value.get(1..).unwrap_or_default();
    (sys::env::os_string(key), sys::env::os_string(value))
}

/// Converts a name or value, which must be Unicode.
fn into_string(s: OsString) -> String {
    s.into_string().unwrap_or_else(|_| not_unicode())
}

#[cold]
#[inline(never)]
#[track_caller]
#[allow(clippy::panic)]
fn not_unicode() -> ! {
    // std's `Vars` panics while yielding a pair that is not Unicode.
    panic!("an environment variable is not valid Unicode")
}
