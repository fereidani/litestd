//! The environment a child gets, and the `PATH` search of the fork path.

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use core::ffi::c_int;
use core::{ffi::c_char, ptr};

use alloc_crate::vec::Vec;

use crate::{io, process::env::CommandEnv, sys::os};

/// The search list of the C library's `execvp` when `PATH` is unset; glibc
/// reports its own, which distributions change, with `confstr`. On the
/// BSDs and Android, the child calls `execvp` itself.
#[cfg(all(target_os = "linux", not(target_env = "musl")))]
const DEFAULT_PATH: &[u8] = b"/bin:/usr/bin";
#[cfg(target_env = "musl")]
const DEFAULT_PATH: &[u8] = b"/usr/local/bin:/bin:/usr/bin";
#[cfg(target_vendor = "apple")]
const DEFAULT_PATH: &[u8] = b"/usr/bin:/bin";

/// Linux's `NAME_MAX`: `execvp` rejects longer program names.
#[cfg(target_os = "linux")]
const NAME_MAX: usize = 255;

/// Linux's `PATH_MAX`: `execvp` skips `PATH` entries this long or longer.
#[cfg(target_os = "linux")]
const PATH_MAX: usize = 4096;

/// Holds the lock that `env::set_var` takes alone, as std's spawn holds its
/// `ENV_LOCK`, so that the environment does not change while it is read or
/// while a child is created from it. Every reader here takes a reference.
pub(super) struct EnvLock {
    #[cfg(feature = "env")]
    _guard: crate::sys::env::EnvGuard,
}

impl EnvLock {
    /// Takes the lock for reading. Without `env`, litestd never changes the
    /// environment, so there is nothing to exclude.
    #[cfg_attr(
        not(feature = "env"),
        allow(clippy::missing_const_for_fn, reason = "locks with `env`")
    )]
    pub(super) fn read() -> Self {
        Self {
            #[cfg(feature = "env")]
            _guard: crate::sys::env::EnvGuard::read(),
        }
    }
}

/// The current environment block, which a child inherits when its command
/// changes nothing.
pub(super) fn current(_: &EnvLock) -> *const *const c_char {
    // SAFETY: litestd changes the environment only under the lock, and
    // `set_var`'s contract rules out other writers.
    unsafe { os::environ() }
}

/// Calls `f` with the name and value of each variable in the current
/// environment, skipping malformed entries.
fn for_each_var(lock: &EnvLock, mut f: impl FnMut(&[u8], &[u8])) {
    // SAFETY: litestd changes the environment only under the lock, which the
    // caller holds, and `set_var`'s contract rules out other writers.
    for var in unsafe { os::Entries::new(lock) } {
        if let Some((name, value)) = split(var) {
            f(name, value);
        }
    }
}

/// Splits `NAME=value` at the first `=` after the name, as glibc and std
/// do: a name is never empty, so a leading `=` belongs to it.
fn split(var: &[u8]) -> Option<(&[u8], &[u8])> {
    let eq = var.get(1..)?.iter().position(|&b| b == b'=')? + 1;
    Some((var.get(..eq)?, var.get(eq + 1..)?))
}

/// The value of `PATH` in the current environment, copied.
pub(super) fn current_path(lock: &EnvLock) -> Option<Vec<u8>> {
    let mut path = None;
    for_each_var(lock, |name, value| {
        if name == b"PATH" {
            path = Some(value.to_vec());
        }
    });
    path
}

/// A child's environment block: the current environment unless cleared,
/// with a command's changes applied, sorted by name as std sorts it.
pub(super) struct EnvBlock {
    /// The `NAME=value` entries, each followed by a NUL byte.
    bytes: Vec<u8>,
    /// Pointers to the entries in `bytes`, sorted by name and ended by null.
    ptrs: Vec<*const c_char>,
    /// Where the value of `PATH` lies in `bytes`, if it is set.
    path: Option<(usize, usize)>,
}

/// An entry of the block: where it starts in `bytes`, and its name length.
#[derive(Clone, Copy)]
struct Entry {
    start: usize,
    name_len: usize,
}

impl EnvBlock {
    /// Builds the block for `env`. A NUL byte in a changed name or value
    /// fails with `InvalidInput`, as it would truncate the entry.
    pub(super) fn new(env: &CommandEnv, lock: &EnvLock) -> io::Result<Self> {
        let (count, size) = Self::size(env, lock);
        let mut bytes = Vec::with_capacity(size);
        let mut inherited = Vec::with_capacity(count);
        if !env.does_clear() {
            for_each_var(lock, |name, value| {
                inherited.push(push_var(&mut bytes, name, value));
            });
        }
        // Sorting keeps equal names in order, so the last one wins, as a
        // later entry overrides an earlier one in std's map.
        inherited.sort_by(|a, b| name(&bytes, *a).cmp(name(&bytes, *b)));
        inherited.dedup_by(|later, earlier| {
            let same = name(&bytes, *later) == name(&bytes, *earlier);
            if same {
                *earlier = *later;
            }
            same
        });
        let mut entries =
            Vec::with_capacity(inherited.len() + env.vars().len());
        let mut inherited = inherited.into_iter().peekable();
        for (key, value) in env.vars() {
            let key = key.as_encoded_bytes();
            // The inherited variables that sort first are kept, one of the
            // same name is replaced.
            while let Some(&entry) = inherited.peek() {
                match name(&bytes, entry).cmp(key) {
                    core::cmp::Ordering::Less => entries.push(entry),
                    core::cmp::Ordering::Equal => {}
                    core::cmp::Ordering::Greater => break,
                }
                inherited.next();
            }
            if let Some(value) = value {
                let value = value.as_encoded_bytes();
                if key.contains(&0) || value.contains(&0) {
                    return Err(os::NUL_IN_DATA);
                }
                entries.push(push_var(&mut bytes, key, value));
            }
        }
        entries.extend(inherited);
        Ok(Self::finish(bytes, &entries))
    }

    /// The number of inherited variables, and the bytes that the entries of
    /// the block take at most, so that nothing grows while it is built.
    fn size(env: &CommandEnv, lock: &EnvLock) -> (usize, usize) {
        let (mut count, mut size) = (0, 0);
        if !env.does_clear() {
            for_each_var(lock, |name, value| {
                count += 1;
                size += name.len() + value.len() + 2;
            });
        }
        for (key, value) in env.vars() {
            size += value.as_ref().map_or(0, |v| key.len() + v.len() + 2);
        }
        (count, size)
    }

    /// Points at the entries in order, once `bytes` no longer moves.
    fn finish(bytes: Vec<u8>, entries: &[Entry]) -> Self {
        let mut ptrs = Vec::with_capacity(entries.len() + 1);
        let mut path = None;
        for &entry in entries {
            let var = bytes.get(entry.start..).unwrap_or_default();
            ptrs.push(var.as_ptr().cast());
            if name(&bytes, entry) == b"PATH" {
                let value = entry.start + entry.name_len + 1;
                let end = var.iter().position(|&b| b == 0).unwrap_or(0);
                path = Some((value, entry.start + end));
            }
        }
        ptrs.push(ptr::null());
        Self { bytes, ptrs, path }
    }

    pub(super) fn as_ptr(&self) -> *const *const c_char {
        self.ptrs.as_ptr()
    }

    /// The child's `PATH`, if set.
    pub(super) fn path(&self) -> Option<&[u8]> {
        let (start, end) = self.path?;
        self.bytes.get(start..end)
    }
}

/// Appends `name=value` and a NUL byte to `bytes`.
fn push_var(bytes: &mut Vec<u8>, name: &[u8], value: &[u8]) -> Entry {
    let start = bytes.len();
    bytes.reserve(name.len() + value.len() + 2);
    bytes.extend_from_slice(name);
    bytes.push(b'=');
    bytes.extend_from_slice(value);
    bytes.push(0);
    Entry {
        start,
        name_len: name.len(),
    }
}

fn name(bytes: &[u8], entry: Entry) -> &[u8] {
    bytes
        .get(entry.start..entry.start + entry.name_len)
        .unwrap_or_default()
}

/// The paths the child of the fork path tries to execute, in order, and the
/// error to report if none works: what the C library's `execvp` would try,
/// computed before forking because the child must not allocate.
pub(super) struct Candidates {
    /// The paths, each followed by a NUL byte.
    pub(super) paths: Vec<u8>,
    /// The error if there is no path to try; the C library's `execvp`
    /// reports its own on the BSDs and Android.
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    pub(super) err: c_int,
    /// Whether the paths come from searching `PATH`, which changes the error
    /// macOS reports when none runs.
    #[cfg(target_vendor = "apple")]
    pub(super) search: bool,
}

impl Candidates {
    /// Lists where macOS's `execvp` looks for `program` with the search list
    /// `path`, the child's `PATH`: an empty entry means `.`, and it skips an
    /// entry that makes a path longer than `MAXPATHLEN`, 1024 bytes.
    #[cfg(target_vendor = "apple")]
    pub(super) fn new(program: &[u8], path: Option<&[u8]>) -> Self {
        let mut paths = Vec::new();
        let search = !program.contains(&b'/');
        if !search {
            paths.extend_from_slice(program);
            paths.push(0);
        } else if !program.is_empty() {
            for dir in path.unwrap_or(DEFAULT_PATH).split(|&b| b == b':') {
                let dir: &[u8] = if dir.is_empty() { b"." } else { dir };
                if dir.len() + program.len() + 2 > 1024 {
                    continue;
                }
                paths.extend_from_slice(dir);
                paths.push(b'/');
                paths.extend_from_slice(program);
                paths.push(0);
            }
        }
        Self {
            paths,
            err: libc::ENOENT,
            search,
        }
    }

    /// Lists where `execvp` looks for `program` with the search list `path`,
    /// the child's `PATH`.
    #[cfg(target_os = "linux")]
    pub(super) fn new(program: &[u8], path: Option<&[u8]>) -> Self {
        let mut paths = Vec::new();
        let err = if program.contains(&b'/') {
            paths.extend_from_slice(program);
            paths.push(0);
            libc::ENOENT
        } else if program.is_empty() {
            libc::ENOENT
        } else if program.len() > NAME_MAX {
            libc::ENAMETOOLONG
        } else {
            let default = path.is_none().then(default_path);
            let path = path.or(default.as_deref()).unwrap_or_default();
            for dir in path.split(|&b| b == b':') {
                if dir.len() >= PATH_MAX {
                    continue;
                }
                // An empty entry means the working directory.
                paths.extend_from_slice(dir);
                if !dir.is_empty() {
                    paths.push(b'/');
                }
                paths.extend_from_slice(program);
                paths.push(0);
            }
            libc::ENOENT
        };
        Self { paths, err }
    }

    /// Holds `program` as it is: on the BSDs and Android the child searches
    /// `PATH` with the C library's `execvp`, as std's child does, for its
    /// exact rules.
    #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
    pub(super) fn new(program: &[u8], _path: Option<&[u8]>) -> Self {
        let mut paths = Vec::with_capacity(program.len() + 1);
        paths.extend_from_slice(program);
        paths.push(0);
        Self { paths }
    }
}

/// The search list of the C library's `execvp` when `PATH` is unset.
#[cfg(target_os = "linux")]
fn default_path() -> Vec<u8> {
    #[cfg(target_env = "gnu")]
    {
        // SAFETY: an empty buffer is not written to; the call returns the
        // size the value needs with its NUL, or 0 on error.
        let len = unsafe { libc::confstr(libc::_CS_PATH, ptr::null_mut(), 0) };
        let mut buf = alloc_crate::vec![0u8; len];
        // SAFETY: `buf` is valid for writes of `len` bytes.
        let r = unsafe {
            libc::confstr(libc::_CS_PATH, buf.as_mut_ptr().cast(), len)
        };
        if len > 0 && r == len {
            buf.pop();
            return buf;
        }
    }
    DEFAULT_PATH.to_vec()
}
