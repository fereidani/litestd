//! `litestd::os::macos::fs` and `os::darwin::fs` against std's on the same
//! files: every `stat` field, and the birth time that `FileTimesExt` sets.

#![cfg(all(target_os = "macos", feature = "fs"))]
#![allow(clippy::unwrap_used, reason = "a failure fails the test")]

mod fs_util;

use core::time::Duration;
use std::os::macos::fs::{FileTimesExt as _, MetadataExt as _};

use fs_util::TempDir;
use litestd::os::{
    darwin::fs::MetadataExt as DarwinMetadataExt, macos::fs::FileTimesExt as _,
    unix::fs::MetadataExt as _,
};

/// Every field, from litestd's metadata.
fn lite_fields(m: &litestd::fs::Metadata) -> [i128; 23] {
    use litestd::os::macos::fs::MetadataExt as _;
    let qspare = m.st_qspare();
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
        m.st_birthtime().into(),
        m.st_birthtime_nsec().into(),
        m.st_blksize().into(),
        m.st_blocks().into(),
        m.st_flags().into(),
        m.st_gen().into(),
        m.st_lspare().into(),
        qspare[0].into(),
        qspare[1].into(),
    ]
}

/// Every field, from std's metadata.
fn std_fields(m: &std::fs::Metadata) -> [i128; 23] {
    let qspare = m.st_qspare();
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
        m.st_birthtime().into(),
        m.st_birthtime_nsec().into(),
        m.st_blksize().into(),
        m.st_blocks().into(),
        m.st_flags().into(),
        m.st_gen().into(),
        m.st_lspare().into(),
        qspare[0].into(),
        qspare[1].into(),
    ]
}

/// The fields every Unix has, through both of litestd's traits.
fn unix_fields(m: &litestd::fs::Metadata) -> [[i128; 3]; 2] {
    [
        [m.dev().into(), m.ino().into(), m.mtime_nsec().into()],
        [
            DarwinMetadataExt::st_dev(m).into(),
            DarwinMetadataExt::st_ino(m).into(),
            DarwinMetadataExt::st_mtime_nsec(m).into(),
        ],
    ]
}

fn assert_same(
    what: &str,
    lite: &litestd::fs::Metadata,
    real: &std::fs::Metadata,
) {
    assert_eq!(lite_fields(lite), std_fields(real), "{what}");
    let [unix, darwin] = unix_fields(lite);
    assert_eq!(unix, darwin, "{what}");
    assert_eq!(
        fs_util::lite_epoch(lite.created().unwrap()),
        fs_util::std_epoch(real.created().unwrap()),
        "{what}: created"
    );
}

#[test]
fn metadata_ext_matches_std() {
    let dir = TempDir::new("macos-metadata");
    let file = dir.join("file");
    std::fs::write(&file, vec![7; 5000]).unwrap();
    // Times before the epoch and with nanoseconds, which macOS reports
    // with negative nanoseconds.
    let time = std::time::UNIX_EPOCH - Duration::new(86_400, 123_456_789);
    let times = std::fs::FileTimes::new()
        .set_accessed(time)
        .set_modified(time + Duration::from_secs(1));
    let handle = std::fs::File::options().write(true).open(&file).unwrap();
    // Miri does not implement `fsetattrlist`; the fields still agree.
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
        assert_eq!(
            fs_util::lite_epoch(meta.accessed().unwrap()),
            Err(Duration::new(86_400, 123_456_789))
        );
    }
    let link = litestd::fs::symlink_metadata(&link).unwrap();
    assert_eq!(link.st_mode() & 0o170_000, 0o120_000);
    assert_eq!(link.st_size(), file.len() as u64);
}

/// `FileTimesExt::set_created` sets the birth time, through a file and a
/// path, as std's does.
#[test]
#[cfg_attr(miri, ignore = "Miri does not implement setattrlist")]
fn set_created_matches_std() {
    let dir = TempDir::new("macos-created");
    let (lite_path, std_path) = (dir.join("lite"), dir.join("std"));
    std::fs::write(&lite_path, b"lite").unwrap();
    std::fs::write(&std_path, b"std").unwrap();
    let born = Duration::new(1_000_000_000, 123_456_789);
    let lite_times = litestd::fs::FileTimes::new()
        .set_created(litestd::time::UNIX_EPOCH + born);
    let std_times =
        std::fs::FileTimes::new().set_created(std::time::UNIX_EPOCH + born);
    assert_eq!(format!("{lite_times:?}"), format!("{std_times:?}"));
    let file = litestd::fs::OpenOptions::new()
        .write(true)
        .open(&lite_path)
        .unwrap();
    file.set_times(lite_times).unwrap();
    let std_file = std::fs::File::options()
        .write(true)
        .open(&std_path)
        .unwrap();
    std_file.set_times(std_times).unwrap();
    let lite = litestd::fs::metadata(&lite_path).unwrap();
    let real = std::fs::metadata(&std_path).unwrap();
    assert_eq!(fs_util::lite_epoch(lite.created().unwrap()), Ok(born));
    assert_eq!(fs_util::std_epoch(real.created().unwrap()), Ok(born));
    // Through the path, a later birth time replaces it.
    let later = born + Duration::from_secs(1);
    litestd::fs::set_times(
        &lite_path,
        litestd::fs::FileTimes::new()
            .set_created(litestd::time::UNIX_EPOCH + later),
    )
    .unwrap();
    let lite = litestd::fs::metadata(&lite_path).unwrap();
    assert_eq!(fs_util::lite_epoch(lite.created().unwrap()), Ok(later));
}

#[test]
#[cfg_attr(miri, ignore = "Miri does not model device files")]
fn metadata_ext_of_a_device_matches_std() {
    let lite = litestd::fs::metadata("/dev/null").unwrap();
    let real = std::fs::metadata("/dev/null").unwrap();
    assert_eq!(lite_fields(&lite), std_fields(&real));
    assert_eq!(lite.st_mode() & 0o170_000, 0o020_000);
}
