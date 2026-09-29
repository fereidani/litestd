//! Helpers shared by the `fs` tests: unique temporary directories, and
//! comparisons of litestd's results with std's.

#![allow(
    dead_code,
    clippy::panic,
    clippy::redundant_pub_crate,
    clippy::unwrap_used,
    reason = "test helpers, used by some tests each, fail the test on errors"
)]

use core::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use std::path::{Path, PathBuf};

/// A directory of its own for one test, which std removes when dropped.
pub(crate) struct TempDir(PathBuf);

/// The directory that temporary directories go in: std's, or on WASI,
/// where std has none, the `/tmp` that `scripts/wasi-run.mjs` provides.
pub(crate) fn temp_base() -> PathBuf {
    if cfg!(target_os = "wasi") {
        PathBuf::from("/tmp")
    } else {
        std::env::temp_dir()
    }
}

/// The id of this process, or 0 on WASI, where std has none and each
/// program gets a directory of its own.
pub(crate) fn process_id() -> u32 {
    if cfg!(target_os = "wasi") {
        0
    } else {
        std::process::id()
    }
}

impl TempDir {
    /// Creates `<temp>/litestd-fs-<name>-<pid>-<n>`.
    pub(crate) fn new(name: &str) -> Self {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        let dir = format!("litestd-fs-{name}-{}-{n}", process_id());
        let path = temp_base().join(dir);
        if path.exists() {
            std::fs::remove_dir_all(&path).unwrap();
        }
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    /// The directory itself, as a string that both std and litestd accept.
    pub(crate) fn path(&self) -> String {
        self.0.to_str().unwrap().to_owned()
    }

    /// `name` inside the directory, as a string.
    pub(crate) fn join(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_owned()
    }

    pub(crate) fn std_path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Tests may leave read-only directories behind; make them writable
        // again so that non-root users can remove them.
        #[cfg(unix)]
        make_writable(&self.0);
        let result = std::fs::remove_dir_all(&self.0);
        assert!(
            result.is_ok() || std::thread::panicking(),
            "cannot remove {}: {result:?}",
            self.0.display()
        );
    }
}

#[cfg(unix)]
fn make_writable(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let _ =
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755));
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            make_writable(&entry.path());
        }
    }
}

/// Whether the tests run as root, which bypasses permission checks.
#[cfg(unix)]
pub(crate) fn is_root() -> bool {
    use std::os::unix::fs::MetadataExt as _;
    // Under Miri `geteuid` reports a made-up user, while the process's own
    // `/proc` entry belongs to the user that really runs it.
    std::fs::metadata("/proc/self").map_or_else(
        // SAFETY: `geteuid` has no preconditions.
        |_| unsafe { libc::geteuid() == 0 },
        |meta| meta.uid() == 0,
    )
}

/// Asserts that a litestd error and a std error agree: kind, OS error code
/// and message.
pub(crate) fn same_error(
    what: &str,
    lite: &litestd::io::Error,
    real: &std::io::Error,
) {
    // Newer std decodes WASI's codes with the Unix table, `ENOTEMPTY`
    // included, as litestd does; stable std 1.98 says `Uncategorized`.
    let real_kind = match (real.raw_os_error(), stable_kind(real.kind())) {
        (Some(55), kind)
            if cfg!(target_os = "wasi") && kind == "Uncategorized" =>
        {
            "DirectoryNotEmpty".to_owned()
        }
        (_, kind) => kind,
    };
    assert_eq!(
        format!("{:?}", lite.kind()),
        real_kind,
        "{what}: error kind ({lite} vs {real})"
    );
    assert_eq!(lite.raw_os_error(), real.raw_os_error(), "{what}: OS error");
    assert_eq!(lite.to_string(), real.to_string(), "{what}: message");
}

/// The name of a std error kind as litestd reports it. litestd has the
/// unstable kinds that stable std decodes OS errors to, but not the newer
/// `InputOutputError` of nightly std, which it reports as `Uncategorized`.
fn stable_kind(kind: std::io::ErrorKind) -> String {
    let name = format!("{kind:?}");
    if name == "InputOutputError" {
        return "Uncategorized".to_owned();
    }
    name
}

/// Asserts that two results agree: both succeed, or both fail alike.
/// Returns the values on success.
#[track_caller]
pub(crate) fn same<T, U>(
    what: &str,
    lite: litestd::io::Result<T>,
    real: std::io::Result<U>,
) -> Option<(T, U)> {
    match (lite, real) {
        (Ok(a), Ok(b)) => Some((a, b)),
        (Err(a), Err(b)) => {
            same_error(what, &a, &b);
            None
        }
        (Ok(_), Err(b)) => panic!("{what}: litestd succeeded, std failed: {b}"),
        (Err(a), Ok(_)) => {
            panic!("{what}: litestd failed: {a:?}, std succeeded")
        }
    }
}

/// A time as its distance from the Unix epoch: `Ok` after, `Err` before.
pub(crate) fn lite_epoch(
    t: litestd::time::SystemTime,
) -> Result<Duration, Duration> {
    t.duration_since(litestd::time::UNIX_EPOCH)
        .map_err(|e| e.duration())
}

pub(crate) fn std_epoch(
    t: std::time::SystemTime,
) -> Result<Duration, Duration> {
    t.duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.duration())
}

/// Asserts that two metadata agree on everything both report.
#[track_caller]
pub(crate) fn same_metadata(
    what: &str,
    lite: &litestd::fs::Metadata,
    real: &std::fs::Metadata,
) {
    let (lt, rt) = (lite.file_type(), real.file_type());
    assert_eq!(
        (lt.is_file(), lt.is_dir(), lt.is_symlink()),
        (rt.is_file(), rt.is_dir(), rt.is_symlink()),
        "{what}: file type"
    );
    assert_eq!(lite.len(), real.len(), "{what}: len");
    assert_eq!(
        lite.permissions().readonly(),
        real.permissions().readonly(),
        "{what}: readonly"
    );
    let times = [
        ("modified", lite.modified(), real.modified()),
        ("accessed", lite.accessed(), real.accessed()),
    ];
    for (name, lite, real) in times {
        if let Some((a, b)) = same(&format!("{what}: {name}"), lite, real) {
            assert_eq!(lite_epoch(a), std_epoch(b), "{what}: {name}");
        }
    }
    same_created(what, lite.created(), real.created());
    #[cfg(unix)]
    assert_eq!(
        unix_fields_lite(lite),
        unix_fields_std(real),
        "{what}: MetadataExt fields"
    );
}

/// Whether std failed to report a birth time only because it asks `statx`
/// for birth times with glibc alone; litestd asks with every C library.
fn std_lacks_birth_time(real: &std::io::Result<std::time::SystemTime>) -> bool {
    const UNAVAILABLE: &str =
        "creation time is not available on this platform currently";
    cfg!(all(target_os = "linux", not(target_env = "gnu")))
        && real.as_ref().is_err_and(|e| {
            e.kind() == std::io::ErrorKind::Unsupported
                && e.to_string() == UNAVAILABLE
        })
}

/// Asserts that two birth times agree or, where std lacks birth times, that
/// litestd's is plausible: after the epoch and not in the future.
#[track_caller]
pub(crate) fn same_created(
    what: &str,
    lite: litestd::io::Result<litestd::time::SystemTime>,
    real: std::io::Result<std::time::SystemTime>,
) {
    if std_lacks_birth_time(&real) {
        let born = lite.unwrap_or_else(|e| panic!("{what}: created: {e}"));
        let now = lite_epoch(litestd::time::SystemTime::now()).unwrap();
        assert!(
            lite_epoch(born).is_ok_and(|t| !t.is_zero() && t <= now),
            "{what}: created {born:?}"
        );
    } else if let Some((a, b)) = same(&format!("{what}: created"), lite, real) {
        assert_eq!(lite_epoch(a), std_epoch(b), "{what}: created");
    }
}

/// std's `Debug` output for `real`, plus the `created` field that litestd
/// adds where std lacks birth times; see [`same_created`].
#[track_caller]
pub(crate) fn std_debug(
    lite: &litestd::fs::Metadata,
    real: &std::fs::Metadata,
) -> String {
    let debug = format!("{real:?}");
    if !std_lacks_birth_time(&real.created()) {
        return debug;
    }
    same_created("Debug", lite.created(), real.created());
    let created = lite.created().unwrap();
    // The field comes last, before the `, .. }` of the non-exhaustive form.
    let end = debug.rfind(", .. }").unwrap();
    let (fields, rest) = debug.split_at(end);
    format!("{fields}, created: {created:?}{rest}")
}

#[cfg(unix)]
fn unix_fields_lite(m: &litestd::fs::Metadata) -> [i128; 16] {
    use litestd::os::unix::fs::MetadataExt;
    [
        m.dev().into(),
        m.ino().into(),
        m.mode().into(),
        m.nlink().into(),
        m.uid().into(),
        m.gid().into(),
        m.rdev().into(),
        m.size().into(),
        m.atime().into(),
        m.atime_nsec().into(),
        m.mtime().into(),
        m.mtime_nsec().into(),
        m.ctime().into(),
        m.ctime_nsec().into(),
        m.blksize().into(),
        m.blocks().into(),
    ]
}

#[cfg(unix)]
fn unix_fields_std(m: &std::fs::Metadata) -> [i128; 16] {
    use std::os::unix::fs::MetadataExt;
    [
        m.dev().into(),
        m.ino().into(),
        m.mode().into(),
        m.nlink().into(),
        m.uid().into(),
        m.gid().into(),
        m.rdev().into(),
        m.size().into(),
        m.atime().into(),
        m.atime_nsec().into(),
        m.mtime().into(),
        m.mtime_nsec().into(),
        m.ctime().into(),
        m.ctime_nsec().into(),
        m.blksize().into(),
        m.blocks().into(),
    ]
}

/// Bytes that make a recognizable, non-repeating file body.
pub(crate) fn pattern(len: usize) -> Vec<u8> {
    let mut state = 0x9E37_79B9_u32;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state.to_le_bytes()[0]
        })
        .collect()
}
