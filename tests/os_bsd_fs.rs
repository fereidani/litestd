//! The `MetadataExt` of `litestd::os::{freebsd, netbsd, openbsd,
//! dragonfly}::fs` against std's on the same files: every `stat` field.

#![cfg(all(
    any(
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ),
    feature = "fs"
))]
#![allow(clippy::unwrap_used, reason = "a failure fails the test")]

mod fs_util;

use fs_util::TempDir;
#[cfg(target_os = "dragonfly")]
use {
    litestd::os::dragonfly::fs::MetadataExt as _,
    std::os::dragonfly::fs::MetadataExt as _,
};
#[cfg(target_os = "freebsd")]
use {
    litestd::os::freebsd::fs::MetadataExt as _,
    std::os::freebsd::fs::MetadataExt as _,
};
#[cfg(target_os = "netbsd")]
use {
    litestd::os::netbsd::fs::MetadataExt as _,
    std::os::netbsd::fs::MetadataExt as _,
};
#[cfg(target_os = "openbsd")]
use {
    litestd::os::openbsd::fs::MetadataExt as _,
    std::os::openbsd::fs::MetadataExt as _,
};

/// Every field that litestd and std both report, from either's metadata.
macro_rules! fields {
    ($m:expr) => {{
        let m = $m;
        let mut fields: Vec<i128> = vec![
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
            m.st_flags().into(),
            m.st_gen().into(),
        ];
        #[cfg(not(target_os = "dragonfly"))]
        fields.extend([
            i128::from(m.st_birthtime()),
            i128::from(m.st_birthtime_nsec()),
        ]);
        #[cfg(target_os = "dragonfly")]
        fields.push(m.st_lspare().into());
        fields
    }};
}

#[test]
fn metadata_fields_match_std() {
    let dir = TempDir::new("bsd-fields");
    let file = dir.join("file");
    std::fs::write(&file, b"some data").unwrap();
    let link = dir.join("link");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    for path in [file, link, dir.path()] {
        let lite = litestd::fs::symlink_metadata(&*path).unwrap();
        let real = std::fs::symlink_metadata(&path).unwrap();
        assert_eq!(fields!(&lite), fields!(&real), "{path}");
    }
}

/// The FreeBSD 12 `stat` that litestd reads has no `st_lspare`: litestd
/// reports 0, where std panics.
#[cfg(target_os = "freebsd")]
#[test]
fn freebsd_lspare_reads_zero() {
    let dir = TempDir::new("bsd-lspare");
    let meta = litestd::fs::metadata(&*dir.path()).unwrap();
    assert_eq!(litestd::os::freebsd::fs::MetadataExt::st_lspare(&meta), 0);
}
