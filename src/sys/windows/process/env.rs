//! The environment of a child: variable names, which Windows compares
//! without regard to case, and the environment block that `CreateProcessW`
//! takes.

use core::cmp::Ordering;

use alloc_crate::vec::Vec;
use windows_sys::Win32::Globalization::{
    CSTR_GREATER_THAN, CSTR_LESS_THAN, CompareStringOrdinal,
};

use super::{
    super::os::environ::{EQUALS, EnvironmentStrings, push_var},
    args::{ensure_no_nuls, push_wide},
};
use crate::{
    ffi::{OsStr, OsString},
    io,
    os::windows::ffi::OsStrExt,
};

/// Compares two variable names as Windows does, ordinally after mapping
/// each code unit to upper case with the system's table, the order in which
/// Windows expects the environment block to be sorted.
fn compare(a: &[u16], b: &[u16]) -> Ordering {
    // Names are far shorter than `i32::MAX` units; a shorter prefix would
    // still compare consistently, and a negative length would be unsound.
    let len = |s: &[u16]| i32::try_from(s.len()).unwrap_or(i32::MAX);
    // SAFETY: each pointer is valid for reads of the length passed with it,
    // and the lengths are not negative, so neither string is read past.
    let result = unsafe {
        CompareStringOrdinal(a.as_ptr(), len(a), b.as_ptr(), len(b), 1)
    };
    // It fails only on invalid arguments.
    debug_assert_ne!(result, 0, "CompareStringOrdinal failed");
    match result {
        CSTR_LESS_THAN => Ordering::Less,
        CSTR_GREATER_THAN => Ordering::Greater,
        _ => Ordering::Equal,
    }
}

/// The name of an environment variable, which keeps its spelling but
/// compares case-insensitively, as Windows does. `Debug` shows both fields,
/// as std's does.
#[derive(Clone, Debug)]
pub(crate) struct EnvKey {
    os_string: OsString,
    /// The name in UTF-16, which the comparisons take.
    utf16: Vec<u16>,
}

impl From<&OsStr> for EnvKey {
    fn from(key: &OsStr) -> Self {
        let mut utf16 = Vec::new();
        push_wide(&mut utf16, key);
        Self {
            os_string: key.to_os_string(),
            utf16,
        }
    }
}

impl AsRef<OsStr> for EnvKey {
    fn as_ref(&self) -> &OsStr {
        &self.os_string
    }
}

impl Ord for EnvKey {
    fn cmp(&self, other: &Self) -> Ordering {
        compare(&self.utf16, &other.utf16)
    }
}

impl PartialOrd for EnvKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for EnvKey {
    fn eq(&self, other: &Self) -> bool {
        self.utf16.len() == other.utf16.len() && self.cmp(other).is_eq()
    }
}

impl Eq for EnvKey {}

impl PartialEq<str> for EnvKey {
    /// As in std, names of different lengths in bytes are never equal.
    fn eq(&self, other: &str) -> bool {
        self.os_string.len() == other.len()
            && Self::from(OsStr::new(other)).cmp(self).is_eq()
    }
}

/// The value of `name` among `changes`, the sorted changes of a command,
/// if the command sets it.
pub(super) fn find<'a>(
    changes: &'a [(EnvKey, Option<OsString>)],
    name: &str,
) -> Option<&'a OsStr> {
    let key = EnvKey::from(OsStr::new(name));
    let i = changes.binary_search_by(|(k, _)| k.cmp(&key)).ok()?;
    changes.get(i)?.1.as_deref()
}

/// Builds the environment block of a child: this process's variables unless
/// `clear`, with the sorted `changes` applied as std's `CommandEnv::capture`
/// applies them, as NUL-terminated `NAME=VALUE` strings sorted as Windows
/// requires, and a final NUL. A name set twice, in any case, keeps its first
/// spelling and its last value.
pub(super) fn env_block(
    clear: bool,
    changes: &[(EnvKey, Option<OsString>)],
) -> io::Result<Vec<u16>> {
    let strings = if clear {
        None
    } else {
        Some(EnvironmentStrings::get()?)
    };
    let mut inherited: Vec<_> =
        strings.iter().flat_map(EnvironmentStrings::vars).collect();
    // Stable, so that a run of equal names keeps the block's order.
    inherited.sort_by(|a, b| compare(a.0, b.0));
    inherited.dedup_by(|later, first| {
        let equal = compare(later.0, first.0).is_eq();
        if equal {
            first.1 = later.1;
        }
        equal
    });
    let capacity = strings.as_ref().map_or(0, |s| s.units().len());
    let mut block = Vec::with_capacity(capacity + 2);
    let mut rest = inherited.iter().peekable();
    for (key, value) in changes {
        let mut name = key.utf16.as_slice();
        // Copies the inherited variables that sort before this one, and
        // drops the one it replaces, whose spelling it takes, as in std.
        while let Some(&&(inherited_name, inherited_value)) = rest.peek() {
            let order = compare(inherited_name, name);
            if order.is_gt() {
                break;
            }
            rest.next();
            if order.is_eq() {
                name = inherited_name;
                break;
            }
            push_var(&mut block, inherited_name, inherited_value);
        }
        if let Some(value) = value {
            ensure_no_nuls(key.as_ref())?;
            ensure_no_nuls(value)?;
            block.reserve(name.len() + value.len() + 2);
            block.extend_from_slice(name);
            block.push(EQUALS);
            block.extend(value.encode_wide());
            block.push(0);
        }
    }
    for &(name, value) in rest {
        push_var(&mut block, name, value);
    }
    // An empty block still needs its two NULs.
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    Ok(block)
}
