//! The CPU quota of the calling process's cgroup, which caps
//! `available_parallelism`. As in std, any failure means no quota, cgroup2 is
//! found only at its standard mount point, and the escapes of
//! `/proc/self/mountinfo` are not undone. Shared helpers stay out of line.

use core::{
    ffi::{CStr, c_int},
    num::NonZero,
};

use alloc_crate::vec::Vec;

use crate::sys::os::errno;

/// Returns the calling process's cgroup CPU quota in whole CPUs, rounded
/// down but at least one, or `NonZero::MAX` if there is none or it cannot be
/// read.
pub(super) fn quota() -> NonZero<usize> {
    // Miri warns about files in `/proc` and emulates its own CPUs, so std
    // skips the quota there as well.
    let quota = if cfg!(miri) { None } else { read_quota() };
    NonZero::new(quota.unwrap_or(usize::MAX)).unwrap_or(NonZero::<usize>::MIN)
}

/// Finds the calling process's cgroup and reads its quota.
fn read_quota() -> Option<usize> {
    let mut text = Vec::new();
    read_to_end(c"/proc/self/cgroup", &mut text)?;
    let (group, v1) = find_group(&text)?;
    if !v1 {
        // The mount point that file-hierarchy(7) defines.
        return Cgroup::new(b"/sys/fs/cgroup", group, false).quota();
    }
    // The usual mount points of cgroups(7) first, then the mount table,
    // which also finds a bind mount of a subtree.
    for mount in [&b"/sys/fs/cgroup/cpu"[..], b"/sys/fs/cgroup/cpu,cpuacct"] {
        if let Some(quota) = Cgroup::new(mount, group, true).quota() {
            return Some(quota);
        }
    }
    let mut mounts = Vec::new();
    read_to_end(c"/proc/self/mountinfo", &mut mounts)?;
    let (mount, group) = find_mount(&mounts, group)?;
    Cgroup::new(mount, group, true).quota()
}

/// Finds the calling process's cgroup in the text of `/proc/self/cgroup`:
/// its path below the root of its hierarchy, and whether that is the v1
/// hierarchy with the `cpu` controller, which wins over cgroup2.
fn find_group(mut text: &[u8]) -> Option<(&[u8], bool)> {
    let mut unified = None;
    // Each round consumes a line, so the loop ends with the text.
    while !text.is_empty() {
        // A line is `id:controllers:/path`, with no controllers for cgroup2.
        let mut line = field(&mut text, b'\n');
        let _id = field(&mut line, b':');
        let controllers = field(&mut line, b':');
        let [b'/', group @ ..] = line else { continue };
        if lists_cpu(controllers) {
            return Some((group, true));
        }
        if controllers.is_empty() {
            unified = Some((group, false));
        }
    }
    unified
}

/// Finds a mount of the v1 hierarchy with the `cpu` controller that shows
/// `group` in the text of `/proc/self/mountinfo`: its mount point, and the
/// rest of `group` below its root, where a bind mount shows a subtree.
fn find_mount<'a>(
    mut text: &'a [u8],
    group: &'a [u8],
) -> Option<(&'a [u8], &'a [u8])> {
    // Each round consumes a line, so the loop ends with the text.
    while !text.is_empty() {
        // A line is `id parent device root mount options [tags] - type
        // source super-options`.
        let mut line = field(&mut text, b'\n');
        for _ in 0..3 {
            field(&mut line, b' ');
        }
        let root = field(&mut line, b' ');
        let mount = field(&mut line, b' ');
        // Each round consumes a field, up to the one that ends the tags.
        while !line.is_empty() && field(&mut line, b' ') != b"-" {}
        let kind = field(&mut line, b' ');
        let _source = field(&mut line, b' ');
        let options = field(&mut line, b' ');
        if kind != b"cgroup" || !lists_cpu(options) {
            continue;
        }
        let Some(root) = root.strip_prefix(b"/") else {
            continue;
        };
        // Compares whole components: `a` holds `a/b` but not `ab`.
        let rest = match group.strip_prefix(root) {
            Some(rest) if root.is_empty() => rest,
            Some([]) => &[],
            Some([b'/', rest @ ..]) => rest,
            _ => continue,
        };
        return Some((mount, rest));
    }
    None
}

/// Returns whether the comma-separated `names` include `cpu`.
fn lists_cpu(mut names: &[u8]) -> bool {
    // Each round consumes a name, so the loop ends with the list.
    while !names.is_empty() {
        if field(&mut names, b',') == b"cpu" {
            return true;
        }
    }
    false
}

/// Takes the first field off `text`, and the `sep` that ends it, if any.
#[inline(never)]
fn field<'a>(text: &mut &'a [u8], sep: u8) -> &'a [u8] {
    let all = *text;
    let end = all.iter().position(|&b| b == sep).unwrap_or(all.len());
    let (head, tail) = all.split_at_checked(end).unwrap_or((all, &[]));
    *text = tail.get(1..).unwrap_or_default();
    head
}

/// A cgroup's directory in a mounted hierarchy.
struct Cgroup {
    /// The directory's path, without a trailing slash.
    path: Vec<u8>,
    /// The length of the hierarchy's mount point, which starts `path`.
    mount: usize,
    /// Whether the hierarchy is v1, with two files for what `cpu.max` holds.
    v1: bool,
}

impl Cgroup {
    /// Locates `group` below the root of the hierarchy mounted at `mount`.
    fn new(mount: &[u8], group: &[u8], v1: bool) -> Self {
        // Leaves room for a slash and the longest file name.
        let mut path = Vec::with_capacity(mount.len() + group.len() + 32);
        path.extend_from_slice(mount);
        if !group.is_empty() {
            path.push(b'/');
            path.extend_from_slice(group);
        }
        Self {
            path,
            mount: mount.len(),
            v1,
        }
    }

    /// Returns the smallest quota of the cgroup and its ancestors in whole
    /// CPUs, or `None` if the directory is not a cgroup of the hierarchy.
    fn quota(mut self) -> Option<usize> {
        // A guessed v1 mount point may be wrong, and only cgroup2
        // directories have `cgroup.controllers`.
        self.open(if self.v1 { c"" } else { c"/cgroup.controllers" })?;
        let mut quota = usize::MAX;
        // Each round moves to the parent, whose path is shorter, and the
        // walk ends at the mount point, the hierarchy's root.
        loop {
            if let Some(own) = self.own_quota() {
                quota = quota.min(own);
            }
            match self.path.iter().rposition(|&b| b == b'/') {
                Some(slash) if slash >= self.mount => self.path.truncate(slash),
                _ => return Some(quota),
            }
        }
    }

    /// Returns the cgroup's own quota in whole CPUs, rounded down, or `None`
    /// if it has none.
    fn own_quota(&mut self) -> Option<usize> {
        let mut buf = [0; 64];
        let (quota, period) = if self.v1 {
            // A quota of -1, which `number` rejects, means none.
            let quota = number(self.read(c"/cpu.cfs_quota_us", &mut buf)?)?;
            let period = number(self.read(c"/cpu.cfs_period_us", &mut buf)?)?;
            (quota, period)
        } else {
            // `max 100000` means none, `150000 100000` one and a half CPUs.
            let mut fields = self.read(c"/cpu.max", &mut buf)?;
            let quota = number(field(&mut fields, b' '))?;
            let period = number(field(&mut fields, b' '))?;
            (quota, period)
        };
        quota.checked_div(period)
    }

    /// Returns the file `name` (see `open`) read into `buf` and trimmed, or
    /// `None` if the read fails or fills `buf`, which no valid contents do.
    fn read<'a>(&mut self, name: &CStr, buf: &'a mut [u8]) -> Option<&'a [u8]> {
        let size = buf.len();
        let text = self.open(name)?.read(buf)?;
        (text.len() < size).then(|| text.trim_ascii())
    }

    /// Opens `name`, a path relative to the directory that starts with a
    /// slash, or the directory itself if `name` is empty.
    #[inline(never)]
    fn open(&mut self, name: &CStr) -> Option<File> {
        let len = self.path.len();
        self.path.extend_from_slice(name.to_bytes_with_nul());
        // A NUL inside the path, which the kernel never reports, fails here.
        let file = CStr::from_bytes_with_nul(&self.path)
            .ok()
            .and_then(File::open);
        self.path.truncate(len);
        file
    }
}

/// Appends the whole file at `path` to `buf`.
fn read_to_end(path: &CStr, buf: &mut Vec<u8>) -> Option<()> {
    const CHUNK: usize = 4096;
    let file = File::open(path)?;
    // Each round appends a chunk, until one that the file does not fill: the
    // kernel generates these files from finite tables, so they end.
    loop {
        let len = buf.len();
        buf.resize(len + CHUNK, 0);
        let read = file.read(buf.get_mut(len..)?)?.len();
        buf.truncate(len + read);
        if read < CHUNK {
            return Some(());
        }
    }
}

/// Parses a decimal number without a sign, or returns `None` if it does not
/// fit a `usize`.
#[inline(never)]
fn number(digits: &[u8]) -> Option<usize> {
    if digits.is_empty() {
        return None;
    }
    digits.iter().try_fold(0_usize, |n, &digit| {
        if !digit.is_ascii_digit() {
            return None;
        }
        n.checked_mul(10)?.checked_add(usize::from(digit - b'0'))
    })
}

/// A file opened for reading, closed on drop.
struct File(c_int);

impl File {
    /// Opens `path` for reading; child processes do not inherit it.
    fn open(path: &CStr) -> Option<Self> {
        // SAFETY: `path` is NUL-terminated, and without `O_CREAT`, `open`
        // reads no mode argument.
        let fd = unsafe {
            libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC)
        };
        // Built only on success: dropping a `File` closes its descriptor.
        if fd < 0 { None } else { Some(Self(fd)) }
    }

    /// Reads until `buf` is full or the file ends, returning the part read.
    fn read<'a>(&self, buf: &'a mut [u8]) -> Option<&'a [u8]> {
        let mut len = 0;
        // Each round fills more of `buf` or retries after a signal, until
        // `buf` is full, the file ends or a read fails.
        while let Some(rest) =
            buf.get_mut(len..).filter(|rest| !rest.is_empty())
        {
            // SAFETY: `rest` is valid for writes of its length.
            let r = unsafe {
                libc::read(self.0, rest.as_mut_ptr().cast(), rest.len())
            };
            match usize::try_from(r) {
                Ok(0) => break,
                Ok(read) => {
                    debug_assert!(read <= rest.len());
                    len += read;
                }
                Err(_) if errno() == libc::EINTR => {}
                Err(_) => return None,
            }
        }
        buf.get(..len)
    }
}

impl Drop for File {
    fn drop(&mut self) {
        // SAFETY: `self` owns the descriptor, which is never used again.
        let r = unsafe { libc::close(self.0) };
        // Errors are ignored, as in std: Linux frees the descriptor even when
        // `close` fails, and only `EBADF` would mean a bug.
        debug_assert!(r == 0 || errno() != libc::EBADF);
    }
}
