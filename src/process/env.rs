//! The environment changes of a `Command`, which every backend shares.

use core::{fmt, slice};

use alloc_crate::{borrow::ToOwned, vec::Vec};

use crate::{
    ffi::{OsStr, OsString},
    sys::process::EnvKey,
};

/// Changes to the environment a child inherits: whether to start from an
/// empty one, and variables to set (`Some`) or remove (`None`).
///
/// The variables are sorted by key in the platform's order, which is the
/// order of std's map; a sorted vector is smaller and, for the few variables
/// a command sets, faster.
#[derive(Default)]
pub(crate) struct CommandEnv {
    clear: bool,
    saw_path: bool,
    vars: Vec<(EnvKey, Option<OsString>)>,
}

impl CommandEnv {
    pub(crate) fn set(&mut self, key: &OsStr, value: &OsStr) {
        let key = self.key(key);
        self.insert(key, Some(value.to_owned()));
    }

    pub(crate) fn remove(&mut self, key: &OsStr) {
        let key = self.key(key);
        // A cleared environment holds nothing to remove, so the change is
        // dropped instead of recorded, as in std.
        if self.clear {
            if let Ok(i) = self.search(&key) {
                self.vars.remove(i);
            }
        } else {
            self.insert(key, None);
        }
    }

    pub(crate) fn clear(&mut self) {
        self.clear = true;
        self.vars.clear();
    }

    /// Whether the child starts from an empty environment.
    pub(crate) const fn does_clear(&self) -> bool {
        self.clear
    }

    /// Whether the child's `PATH` may differ from this process's.
    #[cfg(any(unix, windows))]
    pub(crate) const fn have_changed_path(&self) -> bool {
        self.saw_path || self.clear
    }

    /// Whether the child inherits this process's environment unchanged.
    pub(crate) fn is_unchanged(&self) -> bool {
        !self.clear && self.vars.is_empty()
    }

    /// The changed variables, sorted by key.
    #[cfg(any(unix, windows))]
    pub(crate) fn vars(&self) -> &[(EnvKey, Option<OsString>)] {
        &self.vars
    }

    pub(crate) fn iter(&self) -> EnvIter<'_> {
        EnvIter {
            iter: self.vars.iter(),
        }
    }

    fn key(&mut self, key: &OsStr) -> EnvKey {
        let key = EnvKey::from(key);
        self.saw_path |= key == *"PATH";
        key
    }

    fn search(&self, key: &EnvKey) -> Result<usize, usize> {
        self.vars.binary_search_by(|(k, _)| k.cmp(key))
    }

    /// Sets the change for `key`. A key already present keeps its original
    /// spelling, which matters where keys compare case-insensitively.
    fn insert(&mut self, key: EnvKey, value: Option<OsString>) {
        match self.search(&key) {
            Ok(i) => {
                if let Some((_, slot)) = self.vars.get_mut(i) {
                    *slot = value;
                }
            }
            Err(i) => self.vars.insert(i, (key, value)),
        }
    }
}

#[allow(clippy::missing_fields_in_debug, reason = "std's fields")]
impl fmt::Debug for CommandEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct Vars<'a>(&'a [(EnvKey, Option<OsString>)]);

        impl fmt::Debug for Vars<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_map()
                    .entries(self.0.iter().map(|(k, v)| (k, v)))
                    .finish()
            }
        }

        f.debug_struct("CommandEnv")
            .field("clear", &self.clear)
            .field("vars", &Vars(&self.vars))
            .finish()
    }
}

/// An iterator over the changes of a [`CommandEnv`], sorted by key.
#[derive(Clone)]
pub(crate) struct EnvIter<'a> {
    iter: slice::Iter<'a, (EnvKey, Option<OsString>)>,
}

impl<'a> Iterator for EnvIter<'a> {
    type Item = (&'a OsStr, Option<&'a OsStr>);

    fn next(&mut self) -> Option<Self::Item> {
        let (key, value) = self.iter.next()?;
        Some((key.as_ref(), value.as_deref()))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl ExactSizeIterator for EnvIter<'_> {
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl fmt::Debug for EnvIter<'_> {
    /// Lists the changes left with the backend's key type, as std does.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.iter.clone().map(|(k, v)| (k, v)))
            .finish()
    }
}
