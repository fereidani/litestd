//! `litestd::os::linux::fs::MetadataExt` against std's on the same files:
//! a regular file, a directory, a symbolic link, and a character device.

#![cfg(all(target_os = "linux", feature = "fs"))]

mod fs_util;

use core::time::Duration;
use std::os::linux::fs::MetadataExt as _;

use fs_util::TempDir;
use litestd::os::{linux::fs::MetadataExt as _, unix::fs::MetadataExt as _};

/// Every field, from litestd's metadata.
fn lite_fields(m: &litestd::fs::Metadata) -> [i128; 16] {
    [
        m.st_dev().into(),
        m.st_ino().into(),
        m.st_mode().into(),
        m.st_nlink().into(),
        m.st_uid().into(),
        m.st_gid().into(),
        m.st_rdev().into(),
        m.st_size().into(),
        m.st_atime().into(),
        m.st_atime_nsec().into(),
        m.st_mtime().into(),
        m.st_mtime_nsec().into(),
        m.st_ctime().into(),
        m.st_ctime_nsec().into(),
        m.st_blksize().into(),
        m.st_blocks().into(),
    ]
}

/// Every field, from std's metadata.
fn std_fields(m: &std::fs::Metadata) -> [i128; 16] {
    [
        m.st_dev().into(),
        m.st_ino().into(),
        m.st_mode().into(),
        m.st_nlink().into(),
        m.st_uid().into(),
        m.st_gid().into(),
        m.st_rdev().into(),
        m.st_size().into(),
        m.st_atime().into(),
        m.st_atime_nsec().into(),
        m.st_mtime().into(),
        m.st_mtime_nsec().into(),
        m.st_ctime().into(),
        m.st_ctime_nsec().into(),
        m.st_blksize().into(),
        m.st_blocks().into(),
    ]
}

/// The same fields through the Unix-wide trait.
fn unix_fields(m: &litestd::fs::Metadata) -> [i128; 16] {
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

fn assert_same(
    what: &str,
    lite: &litestd::fs::Metadata,
    real: &std::fs::Metadata,
) {
    assert_eq!(lite_fields(lite), std_fields(real), "{what}");
    assert_eq!(lite_fields(lite), unix_fields(lite), "{what}");
}

#[test]
fn metadata_ext_matches_std() {
    let dir = TempDir::new("linux-metadata");
    let file = dir.join("file");
    std::fs::write(&file, vec![7; 5000]).unwrap();
    // Times before the epoch and with nanoseconds, the same for both reads.
    let time = std::time::UNIX_EPOCH - Duration::new(86_400, 123_456_789);
    let times = std::fs::FileTimes::new()
        .set_accessed(time)
        .set_modified(time + Duration::from_secs(1));
    let handle = std::fs::File::options().write(true).open(&file).unwrap();
    // Miri does not implement `futimens`; the fields still agree.
    if !cfg!(miri) {
        handle.set_times(times).unwrap();
    }
    drop(handle);
    let link = dir.join("link");
    std::os::unix::fs::symlink(&file, &link).unwrap();

    for path in [file.as_str(), dir.path().as_str(), link.as_str()] {
        let lite = litestd::fs::metadata(path).unwrap();
        let real = std::fs::metadata(path).unwrap();
        assert_same(path, &lite, &real);
        let lite = litestd::fs::symlink_metadata(path).unwrap();
        let real = std::fs::symlink_metadata(path).unwrap();
        assert_same(path, &lite, &real);
    }

    let meta = litestd::fs::metadata(&file).unwrap();
    assert_eq!(meta.st_size(), 5000);
    assert_eq!(meta.st_mode() & 0o170_000, 0o100_000);
    if !cfg!(miri) {
        assert_eq!(meta.st_atime(), -86_401);
        assert_eq!(meta.st_atime_nsec(), 876_543_211);
        assert_eq!(meta.st_mtime(), -86_400);
    }
    let link = litestd::fs::symlink_metadata(&link).unwrap();
    assert_eq!(link.st_mode() & 0o170_000, 0o120_000);
    assert_eq!(link.st_size(), file.len() as u64);
}

#[test]
#[cfg_attr(miri, ignore = "Miri does not model device files")]
fn metadata_ext_of_a_device_matches_std() {
    // The memory devices, major 1; whichever is a character device here.
    let devices = ["/dev/zero", "/dev/full", "/dev/null", "/dev/urandom"];
    let device = devices.into_iter().find(|path| {
        std::fs::metadata(path)
            .is_ok_and(|m| m.st_mode() & 0o170_000 == 0o020_000)
    });
    let Some(device) = device else {
        return;
    };
    let lite = litestd::fs::metadata(device).unwrap();
    let real = std::fs::metadata(device).unwrap();
    assert_same(device, &lite, &real);
    assert_eq!(lite.st_mode() & 0o170_000, 0o020_000);
    assert_eq!(lite.st_rdev() >> 8, 1, "{device}");
}
