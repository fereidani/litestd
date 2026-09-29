//! The arguments of the process, read when first requested and converted
//! one by one as they are yielded.

use core::{fmt, ops::Range};

use alloc_crate::string::String;

use super::list::UnitList;
use crate::{ffi::OsString, sys};

/// An iterator over the arguments of a process, yielding a [`String`] value
/// for each argument.
///
/// This struct is created by [`args`]. The first element is traditionally
/// the path of the executable, but it can be set to arbitrary text, so it
/// should not be relied upon for security purposes.
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct Args {
    inner: ArgsOs,
    /// The units of `inner` as a `str`, if they are known to be UTF-8, so
    /// that no argument needs checking on its own.
    text: Option<&'static str>,
}

/// An iterator over the arguments of a process, yielding an [`OsString`]
/// value for each argument.
///
/// This struct is created by [`args_os`]; see [`Args`] for its first
/// element.
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct ArgsOs {
    list: UnitList,
}

/// Returns the arguments that this program was started with, normally
/// passed via the command line.
///
/// The arguments are read when first requested, once, so they reflect the
/// changes the process made to its argument memory before: with glibc from
/// the `argv` that glibc hands to litestd's initializer, as std reads them;
/// with musl and on Android from `/proc/self/cmdline`, where they are empty
/// if `/proc` is not mounted; on macOS from the `argv` that dyld keeps; on
/// the BSDs from `sysctl`. On Windows they come from `GetCommandLineW` at
/// each call, split as std splits it. Each is copied out as it is yielded.
///
/// # Panics
///
/// The returned iterator panics during iteration if an argument is not
/// valid Unicode. Use [`args_os`] to avoid that.
///
/// # Examples
///
/// ```
/// for argument in litestd::env::args() {
///     assert!(!argument.contains('\0'));
/// }
/// ```
pub fn args() -> Args {
    let inner = args_os();
    Args {
        inner,
        text: sys::env::args_text(),
    }
}

/// Returns the arguments that this program was started with, normally
/// passed via the command line, without checking that they are Unicode.
///
/// See [`args`] for how they are read.
///
/// # Examples
///
/// ```
/// let first = litestd::env::args_os().next();
/// assert!(first.is_some());
/// ```
pub fn args_os() -> ArgsOs {
    ArgsOs {
        list: UnitList::new(sys::env::args()),
    }
}

impl Iterator for Args {
    type Item = String;

    fn next(&mut self) -> Option<String> {
        let range = self.inner.list.next_range()?;
        Some(self.string(range))
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for Args {
    #[inline]
    fn len(&self) -> usize {
        self.inner.len()
    }
}

impl DoubleEndedIterator for Args {
    fn next_back(&mut self) -> Option<String> {
        let range = self.inner.list.next_back_range()?;
        Some(self.string(range))
    }
}

impl Args {
    /// Copies out the argument at `range` of the units, which must be
    /// Unicode: straight from `text` if the arguments are known to be.
    fn string(&self, range: Range<usize>) -> String {
        match self.text {
            Some(text) => String::from(text.get(range).unwrap_or_default()),
            None => {
                into_string(sys::env::os_string(self.inner.list.get(range)))
            }
        }
    }
}

#[allow(
    clippy::missing_fields_in_debug,
    reason = "std's format; `text` repeats `inner`"
)]
impl fmt::Debug for Args {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Args")
            .field("inner", &self.inner.list.debug(sys::env::os_string))
            .finish()
    }
}

impl Iterator for ArgsOs {
    type Item = OsString;

    fn next(&mut self) -> Option<OsString> {
        self.list.next().map(sys::env::os_string)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.list.len(), Some(self.list.len()))
    }

    #[inline]
    fn count(self) -> usize {
        self.list.len()
    }

    fn last(mut self) -> Option<OsString> {
        self.next_back()
    }

    fn nth(&mut self, n: usize) -> Option<OsString> {
        // Skipped arguments are not copied out.
        self.list.skip(n);
        self.next()
    }
}

impl ExactSizeIterator for ArgsOs {
    #[inline]
    fn len(&self) -> usize {
        self.list.len()
    }
}

impl DoubleEndedIterator for ArgsOs {
    fn next_back(&mut self) -> Option<OsString> {
        self.list.next_back().map(sys::env::os_string)
    }
}

impl fmt::Debug for ArgsOs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArgsOs")
            .field("inner", &self.list.debug(sys::env::os_string))
            .finish()
    }
}

/// Converts an argument, which must be Unicode.
fn into_string(arg: OsString) -> String {
    arg.into_string().unwrap_or_else(|_| not_unicode())
}

#[cold]
#[inline(never)]
#[track_caller]
#[allow(clippy::panic)]
fn not_unicode() -> ! {
    // std's `Args` panics while yielding an argument that is not Unicode.
    panic!("an argument of the process is not valid Unicode")
}
