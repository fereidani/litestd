//! `litestd::fs::copy` against `std::fs::copy`: contents, byte counts,
//! permissions and errors.

// wasm32-unknown-unknown has no file system: `tests/unsupported.rs`
// compares its errors with std's.
#![cfg(all(feature = "fs", not(target_os = "unknown")))]
// Miri implements none of `copy_file_range`, `sendfile` and `fclonefileat`.
#![cfg(not(miri))]
#![allow(clippy::unwrap_used, reason = "helpers, like tests, fail on errors")]

mod fs_util;

use std::io::{Seek as _, Write as _};
#[cfg(unix)]
use std::thread;

use fs_util::{TempDir, pattern, same};
use litestd::fs as lfs;

/// Copies `from` with litestd to `lite` and with std to `real`, and checks
/// that both agree on the result and the copy.
fn copy_both(from: &str, lite: &str, real: &str) {
    let result = same(from, lfs::copy(from, lite), std::fs::copy(from, real));
    if let Some((a, b)) = result {
        assert_eq!(a, b, "{from}: bytes copied");
        assert_eq!(
            std::fs::read(lite).unwrap(),
            std::fs::read(real).unwrap(),
            "{from}"
        );
        let (lm, rm) = (
            std::fs::metadata(lite).unwrap(),
            std::fs::metadata(real).unwrap(),
        );
        assert_eq!(lm.len(), a);
        assert_eq!(lm.permissions(), rm.permissions(), "{from}: permissions");
    }
}

#[test]
fn copies_files_of_all_sizes() {
    let dir = TempDir::new("copy-sizes");
    for len in [0, 1, 4095, 4096, 65_537, 3 << 20] {
        let from = dir.join(&format!("from-{len}"));
        std::fs::write(&from, pattern(len)).unwrap();
        copy_both(
            &from,
            &dir.join(&format!("lite-{len}")),
            &dir.join(&format!("std-{len}")),
        );
    }
}

/// A sparse file copies to the same contents and length; holes read back as
/// zeros.
#[test]
fn copies_sparse_files() {
    let dir = TempDir::new("copy-sparse");
    let from = dir.join("sparse");
    let mut file = std::fs::File::create(&from).unwrap();
    file.set_len(64 << 20).unwrap();
    for offset in [0, 1 << 20, 33 << 20, (64 << 20) - 5] {
        file.seek(std::io::SeekFrom::Start(offset)).unwrap();
        file.write_all(b"data!").unwrap();
    }
    drop(file);
    copy_both(&from, &dir.join("lite"), &dir.join("std"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let lite = std::fs::metadata(dir.join("lite")).unwrap().blocks();
        let real = std::fs::metadata(dir.join("std")).unwrap().blocks();
        // Both copy through the same system call, so they allocate alike.
        assert_eq!(lite, real, "allocated blocks");
    }
}

#[cfg(unix)]
#[test]
fn preserves_permissions_like_std() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = TempDir::new("copy-perm");
    for mode in [0o644, 0o600, 0o755, 0o777, 0o444, 0o4755] {
        let from = dir.join(&format!("from-{mode:o}"));
        std::fs::write(&from, b"permissions").unwrap();
        std::fs::set_permissions(&from, std::fs::Permissions::from_mode(mode))
            .unwrap();
        let (lite, real) = (
            dir.join(&format!("lite-{mode:o}")),
            dir.join(&format!("std-{mode:o}")),
        );
        copy_both(&from, &lite, &real);
        // Copying over existing files sets their permissions too.
        std::fs::set_permissions(&lite, std::fs::Permissions::from_mode(0o600))
            .unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600))
            .unwrap();
        copy_both(&from, &lite, &real);
        // Writing the data clears setuid and setgid for a process without
        // `CAP_FSETID`, in std's copy as in litestd's, which `copy_both`
        // compares; the permission bits always survive.
        let got = std::fs::metadata(&lite).unwrap().permissions().mode();
        assert_eq!(got & 0o777, mode & 0o777, "{mode:o}");
    }
}

#[test]
fn overwrites_and_truncates_the_destination() {
    let dir = TempDir::new("copy-over");
    let from = dir.join("from");
    std::fs::write(&from, b"short").unwrap();
    for name in ["lite", "std"] {
        std::fs::write(dir.join(name), pattern(100_000)).unwrap();
    }
    copy_both(&from, &dir.join("lite"), &dir.join("std"));
    assert_eq!(std::fs::read(dir.join("lite")).unwrap(), b"short");
}

#[test]
fn errors_match_std() {
    let dir = TempDir::new("copy-errors");
    std::fs::write(dir.join("file"), b"x").unwrap();
    std::fs::create_dir(dir.join("dir")).unwrap();
    let cases = [
        ("missing source", dir.join("missing"), dir.join("to")),
        ("directory source", dir.join("dir"), dir.join("to")),
        (
            "missing destination directory",
            dir.join("file"),
            dir.join("missing/to"),
        ),
        ("directory destination", dir.join("file"), dir.join("dir")),
    ];
    for (what, from, to) in cases {
        same(what, lfs::copy(&from, &to), std::fs::copy(&from, &to));
    }
}

/// Copying to a character device writes the data but neither replaces the
/// device nor changes its permissions. The device is a pseudo-terminal the
/// test opens itself, not a system file such as `/dev/null`.
#[cfg(target_os = "linux")]
#[test]
fn copies_to_a_terminal_without_touching_it() {
    use std::os::{
        fd::FromRawFd as _,
        unix::fs::{
            FileTypeExt as _, OpenOptionsExt as _, PermissionsExt as _,
        },
    };
    let dir = TempDir::new("copy-tty");
    let from = dir.join("from");
    std::fs::write(&from, pattern(10_000)).unwrap();
    std::fs::set_permissions(&from, std::fs::Permissions::from_mode(0o644))
        .unwrap();
    let flags = libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC;
    // SAFETY: `posix_openpt` has no memory preconditions.
    let fd = unsafe { libc::posix_openpt(flags) };
    if fd < 0 {
        // No pseudo-terminals here.
        return;
    }
    // SAFETY: `fd` is a new descriptor that nothing else owns.
    let master = unsafe { std::fs::File::from_raw_fd(fd) };
    let mut name = [0; 64];
    // SAFETY: `fd` is a terminal master, and `name` is valid for writes of
    // its length.
    unsafe {
        assert_eq!(libc::grantpt(fd), 0);
        assert_eq!(libc::unlockpt(fd), 0);
        assert_eq!(libc::ptsname_r(fd, name.as_mut_ptr(), name.len()), 0);
    }
    // SAFETY: `ptsname_r` wrote a NUL-terminated name.
    let slave = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) };
    let slave = slave.to_str().unwrap().to_owned();
    // An open end keeps the terminal alive for the reader below, which
    // drains it so that the copy never waits for room.
    let keep = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open(&slave)
        .unwrap();
    let before = std::fs::metadata(&slave).unwrap().permissions().mode();
    let drain = thread::spawn(move || {
        let mut master = master;
        let mut sink = Vec::new();
        // Ends with `EIO` once every other end of the terminal is closed.
        let _ = std::io::Read::read_to_end(&mut master, &mut sink);
    });
    assert_eq!(lfs::copy(&from, &slave).unwrap(), 10_000);
    let meta = std::fs::symlink_metadata(&slave).unwrap();
    drop(keep);
    drain.join().unwrap();
    assert!(meta.file_type().is_char_device());
    assert_eq!(meta.permissions().mode(), before);
    assert_ne!(before & 0o7777, 0o644, "the modes must differ to tell");
}

/// Files of `/proc` report a size of zero but have contents, which the copy
/// reads to their end.
#[cfg(target_os = "linux")]
#[test]
fn copies_proc_files() {
    let dir = TempDir::new("copy-proc");
    copy_both("/proc/version", &dir.join("lite"), &dir.join("std"));
    assert!(std::fs::metadata(dir.join("lite")).unwrap().len() > 0);
}

/// Runs `copy` into a new FIFO at `fifo`, the test's own, while a thread
/// reads it, and checks that the reader got `data` and that the copy
/// neither replaced the FIFO nor changed its permissions, which root could
/// otherwise do. Returns the copy's result.
#[cfg(unix)]
fn into_fifo<T>(fifo: &str, data: &[u8], copy: impl FnOnce() -> T) -> T {
    use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
    let c_fifo = std::ffi::CString::new(fifo).unwrap();
    // SAFETY: `c_fifo` is a valid C string.
    assert_eq!(unsafe { libc::mkfifo(c_fifo.as_ptr(), 0o600) }, 0);
    let reader = {
        let fifo = fifo.to_owned();
        thread::spawn(move || std::fs::read(fifo).unwrap())
    };
    let result = copy();
    assert_eq!(reader.join().unwrap(), data);
    let meta = std::fs::symlink_metadata(fifo).unwrap();
    assert!(meta.file_type().is_fifo());
    assert_eq!(meta.permissions().mode() & 0o7777, 0o600);
    result
}

/// A FIFO destination takes the read and write loop on Linux: splicing file
/// pages into a pipe would let later writes to the source change data in
/// flight. The FIFO is the test's own, so that a copy that went wrong
/// cannot damage a system file such as `/dev/null`.
#[cfg(unix)]
#[test]
fn copies_into_a_fifo() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = TempDir::new("copy-fifo");
    let from = dir.join("from");
    let data = pattern(300_000);
    std::fs::write(&from, &data).unwrap();
    std::fs::set_permissions(&from, std::fs::Permissions::from_mode(0o644))
        .unwrap();
    let fifo = dir.join("lite");
    let lite = into_fifo(&fifo, &data, || lfs::copy(&from, &fifo));
    #[cfg(not(target_vendor = "apple"))]
    assert_eq!(lite.unwrap(), 300_000);
    // `fcopyfile` ends by truncating the FIFO to the bytes it wrote, which
    // the file system may refuse; std's copy then fails alike.
    #[cfg(target_vendor = "apple")]
    {
        let fifo = dir.join("std");
        let real = into_fifo(&fifo, &data, || std::fs::copy(&from, &fifo));
        if let Some(copied) = same("copy into a FIFO", lite, real) {
            assert_eq!(copied, (300_000, 300_000));
        }
    }
}

/// The source's cursor does not matter: the whole file is copied.
#[test]
fn copies_from_the_start() {
    let dir = TempDir::new("copy-start");
    let from = dir.join("from");
    let mut file = std::fs::File::create(&from).unwrap();
    file.write_all(b"whole file").unwrap();
    drop(file);
    assert_eq!(lfs::copy(&from, dir.join("to")).unwrap(), 10);
    assert_eq!(std::fs::read(dir.join("to")).unwrap(), b"whole file");
}

/// Between file systems `copy_file_range` fails with `EXDEV`, and the copy
/// continues with `sendfile`.
#[cfg(target_os = "linux")]
#[test]
fn copies_between_file_systems() {
    use std::os::unix::fs::MetadataExt as _;
    let shm = std::path::Path::new("/dev/shm");
    let dir = TempDir::new("copy-xdev");
    let other =
        shm.join(format!("litestd-fs-copy-xdev-{}", std::process::id()));
    let Ok(shm_meta) = std::fs::metadata(shm) else {
        return;
    };
    if shm_meta.dev() == std::fs::metadata(dir.path()).unwrap().dev()
        || std::fs::create_dir(&other).is_err()
    {
        return;
    }
    // Removes `other` from /dev/shm even when an assertion below fails.
    let _cleanup = RemoveOnDrop(&other);
    let from = dir.join("from");
    std::fs::write(&from, pattern(1 << 20)).unwrap();
    let (lite, real) = (other.join("lite"), other.join("std"));
    copy_both(&from, lite.to_str().unwrap(), real.to_str().unwrap());
    copy_both(
        real.to_str().unwrap(),
        &dir.join("back-lite"),
        &dir.join("back-std"),
    );
    std::fs::remove_dir_all(&other).unwrap();
}

/// Removes a directory tree when dropped, also while a failing assertion
/// unwinds, so a test never leaves files outside its temp directory.
#[cfg(target_os = "linux")]
struct RemoveOnDrop<'a>(&'a std::path::Path);

#[cfg(target_os = "linux")]
impl Drop for RemoveOnDrop<'_> {
    fn drop(&mut self) {
        // Best-effort cleanup: the test has already asserted what it checks,
        // and a failure here must not mask the test's own failure.
        let _ = std::fs::remove_dir_all(self.0);
    }
}
