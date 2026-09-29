//! Iterating over the components of a path.

use core::{cmp::Ordering, fmt, iter::FusedIterator};

use super::{
    HAS_PREFIXES, MAIN_SEPARATOR_STR, Path, Prefix, PrefixComponent, is_sep,
    is_sep_byte, os_str_from_piece, path_from_piece, prefix::parse_prefix,
};
use crate::ffi::OsStr;

/// How far one end of a [`Components`] iterator has got. Front to back, a
/// path is a prefix (on Windows), a start (a root, a leading `.`, or nothing)
/// and a body of normal components; each end moves towards the other.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum State {
    /// `C:` and the like.
    Prefix,
    /// `/`, `.`, or nothing.
    StartDir,
    /// `foo/bar/baz`.
    Body,
    Done,
}

/// A single component of a path, roughly a substring between separators, as
/// yielded by [`Path::components`].
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Component<'a> {
    /// A Windows path prefix, e.g., `C:` or `\\server\share`; see [`Prefix`].
    /// Does not occur on Unix.
    Prefix(PrefixComponent<'a>),

    /// The root directory, after any prefix and before anything else.
    RootDir,

    /// A reference to the current directory, i.e., `.`.
    CurDir,

    /// A reference to the parent directory, i.e., `..`.
    ParentDir,

    /// A normal component, e.g., `a` and `b` in `a/b`.
    Normal(&'a OsStr),
}

impl<'a> Component<'a> {
    /// Extracts the underlying [`OsStr`] slice.
    #[must_use = "`self` will be dropped if the result is not used"]
    pub fn as_os_str(self) -> &'a OsStr {
        match self {
            Component::Prefix(p) => p.as_os_str(),
            Component::RootDir => OsStr::new(MAIN_SEPARATOR_STR),
            Component::CurDir => OsStr::new("."),
            Component::ParentDir => OsStr::new(".."),
            Component::Normal(path) => path,
        }
    }
}

impl AsRef<OsStr> for Component<'_> {
    #[inline]
    fn as_ref(&self) -> &OsStr {
        self.as_os_str()
    }
}

impl AsRef<Path> for Component<'_> {
    #[inline]
    fn as_ref(&self) -> &Path {
        Path::new(self.as_os_str())
    }
}

/// An iterator over the [`Component`]s of a [`Path`], created by
/// [`Path::components`].
#[derive(Clone)]
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct Components<'a> {
    /// The part of the path left to parse.
    path: &'a [u8],

    /// The prefix, as parsed from the whole path.
    #[cfg(windows)]
    prefix: Option<Prefix<'a>>,

    /// Whether a separator follows the prefix or starts the path. Most Windows
    /// prefixes root a path without one: `\\server\share` is rooted too.
    has_physical_root: bool,

    front: State,
    back: State,
}

/// An iterator over the [`Component`]s of a [`Path`], as [`OsStr`] slices,
/// created by [`Path::iter`].
#[derive(Clone)]
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct Iter<'a> {
    pub(super) inner: Components<'a>,
}

impl fmt::Debug for Components<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct DebugHelper<'a>(&'a Path);

        impl fmt::Debug for DebugHelper<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_list().entries(self.0.components()).finish()
            }
        }

        f.debug_tuple("Components")
            .field(&DebugHelper(self.as_path()))
            .finish()
    }
}

impl<'a> Components<'a> {
    /// Starts iterating over the components of `path`.
    pub(super) fn new(path: &'a Path) -> Self {
        let bytes = path.as_os_str().as_encoded_bytes();
        let prefix = parse_prefix(path.as_os_str());
        let after_prefix =
            prefix.and_then(|p| bytes.get(p.len()..)).unwrap_or(bytes);
        Self {
            path: bytes,
            #[cfg(windows)]
            prefix,
            has_physical_root: after_prefix
                .first()
                .is_some_and(|&b| is_sep_byte(b)),
            // Without prefixes, skip the state that parses them.
            front: if HAS_PREFIXES {
                State::Prefix
            } else {
                State::StartDir
            },
            back: State::Body,
        }
    }

    /// The prefix of the whole path.
    #[cfg(windows)]
    #[inline]
    pub(super) const fn prefix(&self) -> Option<Prefix<'a>> {
        self.prefix
    }

    /// The prefix of the whole path, which Unix never has.
    #[cfg(not(windows))]
    #[inline]
    #[allow(clippy::unused_self, reason = "the same call on every platform")]
    pub(super) const fn prefix(&self) -> Option<Prefix<'a>> {
        None
    }

    /// The length of the prefix, or 0.
    #[inline]
    pub(super) fn prefix_len(&self) -> usize {
        self.prefix().map_or(0, |p| p.len())
    }

    /// Whether the prefix is verbatim, which turns off normalization.
    #[inline]
    pub(super) fn prefix_verbatim(&self) -> bool {
        self.prefix().is_some_and(|p| p.is_verbatim())
    }

    /// The length of the prefix the front has not consumed yet.
    #[inline]
    pub(super) fn prefix_remaining(&self) -> usize {
        if self.front == State::Prefix {
            self.prefix_len()
        } else {
            0
        }
    }

    /// The length of the path left before the body: the prefix, root and
    /// leading `.` that the front has not consumed yet.
    #[inline]
    fn len_before_body(&self) -> usize {
        let start = self.front <= State::StartDir;
        let root = start && self.has_physical_root;
        let cur_dir = start && self.include_cur_dir();
        self.prefix_remaining() + usize::from(root) + usize::from(cur_dir)
    }

    /// Whether the two ends have met.
    #[inline]
    fn finished(&self) -> bool {
        self.front == State::Done
            || self.back == State::Done
            || self.front > self.back
    }

    /// The bytes of the path left to parse.
    #[inline]
    pub(super) const fn remaining(&self) -> &'a [u8] {
        self.path
    }

    /// Extracts a slice corresponding to the portion of the path remaining for
    /// iteration.
    #[must_use]
    pub fn as_path(&self) -> &'a Path {
        let mut comps = self.clone();
        if comps.front == State::Body {
            comps.trim_left();
        }
        if comps.back == State::Body {
            comps.trim_right();
        }
        // SAFETY: only whole components, separators, roots and prefixes
        // were cut off either end, so the rest is bounded by separators,
        // dots, the prefix or the ends of the path.
        unsafe { path_from_piece(comps.path) }
    }

    /// Whether the whole path is rooted.
    pub(super) fn has_root(&self) -> bool {
        self.has_physical_root
            || self.prefix().is_some_and(|p| p.has_implicit_root())
    }

    /// Whether the normalized path starts with `.`.
    fn include_cur_dir(&self) -> bool {
        if self.has_root() {
            return false;
        }
        match self.path.get(self.prefix_remaining()..) {
            Some([b'.']) => true,
            Some([b'.', b, ..]) => is_sep(*b, self.prefix_verbatim()),
            _ => false,
        }
    }

    /// Classifies the bytes of one component. `.` and empty components are
    /// normalized away, except that verbatim paths keep `.`.
    fn parse_single_component<'b>(
        &self,
        comp: &'b [u8],
    ) -> Option<Component<'b>> {
        match comp {
            b"." if self.prefix_verbatim() => Some(Component::CurDir),
            b"." | b"" => None,
            b".." => Some(Component::ParentDir),
            // SAFETY: `comp` lies between separators or the ends of the
            // body.
            _ => Some(Component::Normal(unsafe { os_str_from_piece(comp) })),
        }
    }

    /// Parses the component at the front of the body, returning its length with
    /// its separator, and the component unless it is normalized away.
    fn parse_next_component(&self) -> (usize, Option<Component<'a>>) {
        debug_assert!(self.front == State::Body);
        let verbatim = self.prefix_verbatim();
        let mut parts = self.path.split(|&b| is_sep(b, verbatim));
        let comp = parts.next().unwrap_or_default();
        let sep = usize::from(comp.len() < self.path.len());
        (comp.len() + sep, self.parse_single_component(comp))
    }

    /// Parses the component at the back of the body, as
    /// [`Components::parse_next_component`] does at the front.
    fn parse_next_component_back(&self) -> (usize, Option<Component<'a>>) {
        debug_assert!(self.back == State::Body);
        let body = self.path.get(self.len_before_body()..).unwrap_or_default();
        let verbatim = self.prefix_verbatim();
        let comp = body.rsplit(|&b| is_sep(b, verbatim)).next();
        let comp = comp.unwrap_or_default();
        let sep = usize::from(comp.len() < body.len());
        (comp.len() + sep, self.parse_single_component(comp))
    }

    /// Drops the components normalized away from the front of the body.
    fn trim_left(&mut self) {
        // Each round drops at least one byte: a separator, or a `.` that
        // ends the path.
        while !self.path.is_empty() {
            let (size, comp) = self.parse_next_component();
            if comp.is_some() {
                return;
            }
            debug_assert!(size > 0);
            self.path = self.path.get(size..).unwrap_or_default();
        }
    }

    /// Drops the components normalized away from the back of the body.
    fn trim_right(&mut self) {
        // Each round drops at least one byte, as in `trim_left`.
        while self.path.len() > self.len_before_body() {
            let (size, comp) = self.parse_next_component_back();
            if comp.is_some() {
                return;
            }
            debug_assert!(size > 0);
            self.path = drop_back(self.path, size);
        }
    }

    /// Yields the prefix from the front, as the bytes it spans.
    fn next_prefix(&mut self) -> Option<Component<'a>> {
        let prefix = self.prefix()?;
        let (raw, rest) = self
            .path
            .split_at_checked(prefix.len())
            .unwrap_or((self.path, &[]));
        self.path = rest;
        // SAFETY: the prefix ends before a separator, after a drive colon,
        // or at the end of the path.
        let raw = unsafe { os_str_from_piece(raw) };
        Some(Component::Prefix(PrefixComponent {
            raw,
            parsed: prefix,
        }))
    }
}

/// Drops the last `n` bytes of `bytes`.
#[inline]
fn drop_back(bytes: &[u8], n: usize) -> &[u8] {
    bytes
        .get(..bytes.len().saturating_sub(n))
        .unwrap_or_default()
}

/// Implements `AsRef<Path>` and `AsRef<OsStr>` for the iterators that have
/// `as_path`.
macro_rules! impl_as_ref {
    ($($ty:ty),+) => {$(
        impl AsRef<Path> for $ty {
            #[inline]
            fn as_ref(&self) -> &Path {
                self.as_path()
            }
        }

        impl AsRef<OsStr> for $ty {
            #[inline]
            fn as_ref(&self) -> &OsStr {
                self.as_path().as_os_str()
            }
        }
    )+};
}

impl_as_ref!(Components<'_>, Iter<'_>);

impl<'a> Iterator for Components<'a> {
    type Item = Component<'a>;

    fn next(&mut self) -> Option<Component<'a>> {
        // Each round consumes bytes or moves the front on, so this ends.
        while !self.finished() {
            match self.front {
                State::Body if !self.path.is_empty() => {
                    let (size, comp) = self.parse_next_component();
                    self.path = self.path.get(size..).unwrap_or_default();
                    if comp.is_some() {
                        return comp;
                    }
                }
                State::Body => self.front = State::Done,
                State::StartDir => {
                    self.front = State::Body;
                    if self.has_physical_root {
                        debug_assert_ne!(self.path, []);
                        self.path = self.path.get(1..).unwrap_or_default();
                        return Some(Component::RootDir);
                    } else if let Some(p) = self.prefix() {
                        if p.has_implicit_root() && !p.is_verbatim() {
                            return Some(Component::RootDir);
                        }
                    } else if self.include_cur_dir() {
                        debug_assert_ne!(self.path, []);
                        self.path = self.path.get(1..).unwrap_or_default();
                        return Some(Component::CurDir);
                    }
                }
                State::Prefix => {
                    self.front = State::StartDir;
                    if let Some(prefix) = self.next_prefix() {
                        return Some(prefix);
                    }
                }
                State::Done => break,
            }
        }
        None
    }
}

impl<'a> DoubleEndedIterator for Components<'a> {
    fn next_back(&mut self) -> Option<Component<'a>> {
        // Each round consumes bytes or moves the back on, so this ends.
        while !self.finished() {
            match self.back {
                State::Body if self.path.len() > self.len_before_body() => {
                    let (size, comp) = self.parse_next_component_back();
                    self.path = drop_back(self.path, size);
                    if comp.is_some() {
                        return comp;
                    }
                }
                State::Body => self.back = State::StartDir,
                State::StartDir => {
                    self.back = if HAS_PREFIXES {
                        State::Prefix
                    } else {
                        State::Done
                    };
                    if self.has_physical_root {
                        self.path = drop_back(self.path, 1);
                        return Some(Component::RootDir);
                    } else if let Some(p) = self.prefix() {
                        if p.has_implicit_root() && !p.is_verbatim() {
                            return Some(Component::RootDir);
                        }
                    } else if self.include_cur_dir() {
                        self.path = drop_back(self.path, 1);
                        return Some(Component::CurDir);
                    }
                }
                State::Prefix => {
                    self.back = State::Done;
                    // Whatever the front has left is the prefix: std yields
                    // it all, `.` included in `C:.`.
                    let prefix = self.prefix()?;
                    // SAFETY: the rest of the path starts at the start of
                    // the path and ends at the end of the prefix or after a
                    // dot the body did not consume.
                    let raw = unsafe { os_str_from_piece(self.path) };
                    let prefix = PrefixComponent {
                        raw,
                        parsed: prefix,
                    };
                    return Some(Component::Prefix(prefix));
                }
                State::Done => break,
            }
        }
        None
    }
}

impl FusedIterator for Components<'_> {}

impl PartialEq for Components<'_> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        // Fast path for exact matches, e.g. for hash map lookups. The prefix
        // and root are covered by `path`, except for whether the prefix is
        // verbatim, which changes how the rest parses.
        if self.path.len() == other.path.len()
            && self.front == other.front
            && self.back == State::Body
            && other.back == State::Body
            && self.prefix_verbatim() == other.prefix_verbatim()
            && self.path == other.path
        {
            return true;
        }
        // Compare back to front, since absolute paths often share long
        // prefixes.
        Iterator::eq(self.clone().rev(), other.clone().rev())
    }
}

impl Eq for Components<'_> {}

impl PartialOrd for Components<'_> {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Components<'_> {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        compare_components(self.clone(), other.clone())
    }
}

/// Compares paths component by component.
///
/// Without prefixes, the shared leading bytes up to the last separator before
/// the first difference are skipped, which cannot change the result; backing
/// up to a separator keeps a `.` or `..` next to the difference.
pub(super) fn compare_components(
    mut left: Components<'_>,
    mut right: Components<'_>,
) -> Ordering {
    if left.prefix().is_none()
        && right.prefix().is_none()
        && left.front == right.front
    {
        let common = left
            .path
            .iter()
            .zip(right.path)
            .take_while(|(a, b)| a == b)
            .count();
        if common == left.path.len() && common == right.path.len() {
            return Ordering::Equal;
        }
        let verbatim = left.prefix_verbatim();
        let shared = left.path.get(..common).unwrap_or_default();
        if let Some(sep) = shared.iter().rposition(|&b| is_sep(b, verbatim)) {
            let start = sep + 1;
            left.path = left.path.get(start..).unwrap_or_default();
            left.front = State::Body;
            right.path = right.path.get(start..).unwrap_or_default();
            right.front = State::Body;
        }
    }
    Iterator::cmp(left, right)
}

/// Iterates through `iter` while it matches `prefix`, returning `iter` after
/// the whole of `prefix`, or `None` if `prefix` is not a prefix of `iter`.
pub(super) fn iter_after<'a, 'b, I, J>(mut iter: I, mut prefix: J) -> Option<I>
where
    I: Iterator<Item = Component<'a>> + Clone,
    J: Iterator<Item = Component<'b>>,
{
    // Each round consumes one component of both, so this ends.
    loop {
        let mut iter_next = iter.clone();
        match (iter_next.next(), prefix.next()) {
            (Some(ref x), Some(ref y)) if x == y => (),
            (Some(_) | None, Some(_)) => return None,
            (_, None) => return Some(iter),
        }
        iter = iter_next;
    }
}

impl fmt::Debug for Iter<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct DebugHelper<'a>(&'a Path);

        impl fmt::Debug for DebugHelper<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_list().entries(self.0.iter()).finish()
            }
        }

        f.debug_tuple("Iter")
            .field(&DebugHelper(self.as_path()))
            .finish()
    }
}

impl<'a> Iter<'a> {
    /// Extracts a slice corresponding to the portion of the path remaining for
    /// iteration.
    #[must_use]
    #[inline]
    pub fn as_path(&self) -> &'a Path {
        self.inner.as_path()
    }
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a OsStr;

    #[inline]
    fn next(&mut self) -> Option<&'a OsStr> {
        self.inner.next().map(Component::as_os_str)
    }
}

impl<'a> DoubleEndedIterator for Iter<'a> {
    #[inline]
    fn next_back(&mut self) -> Option<&'a OsStr> {
        self.inner.next_back().map(Component::as_os_str)
    }
}

impl FusedIterator for Iter<'_> {}

/// An iterator over [`Path`] and its ancestors, created by
/// [`Path::ancestors`].
#[derive(Copy, Clone, Debug)]
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct Ancestors<'a> {
    pub(super) next: Option<&'a Path>,
}

// std's `Ancestors` is `Copy` and an iterator.
#[allow(clippy::copy_iterator)]
impl<'a> Iterator for Ancestors<'a> {
    type Item = &'a Path;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let next = self.next;
        self.next = next.and_then(Path::parent);
        next
    }
}

impl FusedIterator for Ancestors<'_> {}
