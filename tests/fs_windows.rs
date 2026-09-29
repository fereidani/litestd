//! Windows-specific behavior of `litestd::fs` and the `os::windows::fs` and
//! `os::windows::io` extensions, compared with std on the same files:
//! path forms, read-only files, the metadata extension, links, locks and
//! handles.

#![cfg(all(windows, feature = "fs"))]

mod fs_util;
mod fs_windows_util;

use core::time::Duration;
use std::{
    io::Read as _,
    os::windows::{
        ffi::{OsStrExt as _, OsStringExt as _},
        fs::{
            FileExt as _, FileTimesExt as _, FileTypeExt as _,
            MetadataExt as _, OpenOptionsExt as _,
        },
        io::{FromRawHandle as _, IntoRawHandle as _},
    },
    path::Path,
};

use fs_util::{
    TempDir, lite_epoch, same, same_error, same_metadata, std_epoch,
};
use fs_windows_util::{
    ERROR_PRIVILEGE_NOT_HELD, FILE_FLAG_BACKUP_SEMANTICS, junction, symlink,
};
use litestd::{
    fs as lfs,
    io::{self as lio, Read as _, Write as _},
    os::windows::{
        ffi::{OsStrExt as _, OsStringExt as _},
        fs::{
            FileExt as _, FileTimesExt as _, FileTypeExt as _,
            MetadataExt as _, OpenOptionsExt as _,
        },
        io::{
            AsHandle as _, AsRawHandle as _, BorrowedHandle,
            FromRawHandle as _, HandleOrInvalid, HandleOrNull,
            IntoRawHandle as _, OwnedHandle,
        },
    },
};

const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
const ERROR_SHARING_VIOLATION: i32 = 32;
const ERROR_LOCK_VIOLATION: i32 = 33;
const ERROR_NOT_LOCKED: i32 = 158;
/// `SECURITY_IDENTIFICATION` from `winbase.h`.
const SECURITY_IDENTIFICATION: u32 = 0x0001_0000;

/// The five `MetadataExt` fields of litestd's metadata.
fn lite_ext(m: &lfs::Metadata) -> [u64; 5] {
    [
        m.file_attributes().into(),
        m.creation_time(),
        m.last_access_time(),
        m.last_write_time(),
        m.file_size(),
    ]
}

/// The five `MetadataExt` fields of std's metadata.
fn std_ext(m: &std::fs::Metadata) -> [u64; 5] {
    [
        m.file_attributes().into(),
        m.creation_time(),
        m.last_access_time(),
        m.last_write_time(),
        m.file_size(),
    ]
}

/// Asserts that the metadata agree, the extension fields included.
#[track_caller]
fn same_metadata_ext(
    what: &str,
    lite: &lfs::Metadata,
    real: &std::fs::Metadata,
) {
    same_metadata(what, lite, real);
    assert_eq!(lite_ext(lite), std_ext(real), "{what}: MetadataExt fields");
    let (lt, rt) = (lite.file_type(), real.file_type());
    assert_eq!(
        (lt.is_symlink_dir(), lt.is_symlink_file()),
        (rt.is_symlink_dir(), rt.is_symlink_file()),
        "{what}: FileTypeExt"
    );
}

#[test]
fn verbatim_paths_reach_the_same_files() {
    let dir = TempDir::new("verbatim");
    let plain = dir.join("file.txt");
    let verbatim = format!(r"\\?\{plain}");
    lfs::write(&verbatim, b"verbatim").unwrap();
    assert_eq!(std::fs::read(&plain).unwrap(), b"verbatim");
    assert_eq!(lfs::read(&plain).unwrap(), b"verbatim");
    let (lite, real) = same(
        "verbatim metadata",
        lfs::metadata(&verbatim),
        std::fs::metadata(&verbatim),
    )
    .unwrap();
    same_metadata_ext("verbatim", &lite, &real);
    // Verbatim paths are taken literally: `/` is not a separator.
    let slash = format!(r"\\?\{}/file.txt", dir.path());
    same(
        "verbatim with slash",
        lfs::metadata(&slash),
        std::fs::metadata(&slash),
    );
    // Forward slashes are separators in ordinary paths.
    let forward = plain.replace('\\', "/");
    let (lite, real) = same(
        "forward slashes",
        lfs::metadata(&forward),
        std::fs::metadata(&forward),
    )
    .unwrap();
    same_metadata_ext("forward slashes", &lite, &real);
}

#[test]
fn long_paths_work_beyond_max_path() {
    let dir = TempDir::new("long");
    let component = "d".repeat(60);
    let mut deep = dir.path();
    for _ in 0..6 {
        deep.push('\\');
        deep.push_str(&component);
    }
    assert!(deep.len() > 300, "the path must exceed MAX_PATH");
    lfs::create_dir_all(&deep).unwrap();
    assert!(std::fs::metadata(&deep).unwrap().is_dir());
    let file = format!(r"{deep}\{}.txt", "f".repeat(100));
    lfs::write(&file, b"long").unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"long");
    let (lite, real) = same(
        "long metadata",
        lfs::metadata(&file),
        std::fs::metadata(&file),
    )
    .unwrap();
    same_metadata_ext("long", &lite, &real);
    let lite_names: Vec<_> = lfs::read_dir(&deep)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(lite_names, [format!("{}.txt", "f".repeat(100))]);
    let (lite, real) = same(
        "long canonicalize",
        lfs::canonicalize(&file),
        std::fs::canonicalize(&file),
    )
    .unwrap();
    assert_eq!(lite.to_str(), real.to_str());
    lfs::remove_dir_all(dir.join(&component)).unwrap();
    assert!(!Path::new(&dir.join(&component)).exists());
}

#[test]
fn relative_paths_resolve_like_std() {
    // The current directory is shared by all tests; only read it.
    let cwd = std::env::current_dir().unwrap();
    for path in [
        "Cargo.toml",
        r".\Cargo.toml",
        "src",
        r"src\..\Cargo.toml",
        "missing.txt",
    ] {
        let lite = lfs::metadata(path);
        let real = std::fs::metadata(path);
        if let Some((lite, real)) = same(path, lite, real) {
            same_metadata_ext(path, &lite, &real);
        }
        let lite = litestd::path::absolute(path).unwrap();
        let real = std::path::absolute(path).unwrap();
        assert_eq!(lite.to_str(), real.to_str(), "absolute({path})");
        assert!(real.starts_with(&cwd) || !real.is_relative());
    }
}

#[test]
fn absolute_matches_std() {
    let inputs = [
        "foo",
        r"foo\..\bar",
        "C:",
        "C:foo",
        r"C:\a\.\b\..\c",
        "C:/a/b",
        r"\\server\share\x\..\y",
        r"\\?\C:\a\..\b",
        r"\\?\C:/not/a/separator",
        r"\\.\C:\x",
        "NUL",
        r"C:\dir\file.",
        "  spaces  ",
    ];
    for input in inputs {
        let (lite, real) = same(
            input,
            litestd::path::absolute(input),
            std::path::absolute(input),
        )
        .unwrap();
        assert_eq!(lite.to_str(), real.to_str(), "absolute({input:?})");
    }
    same_error(
        "empty",
        &litestd::path::absolute("").unwrap_err(),
        &std::path::absolute("").unwrap_err(),
    );
    same_error(
        "verbatim NUL",
        &litestd::path::absolute("\\\\?\\C:\\a\0b").unwrap_err(),
        &std::path::absolute("\\\\?\\C:\\a\0b").unwrap_err(),
    );
}

#[test]
fn interior_nul_is_invalid_input() {
    let dir = TempDir::new("nul");
    let path = dir.join("a\0b");
    same_error(
        "open",
        &lfs::File::create(&path).unwrap_err(),
        &std::fs::File::create(&path).unwrap_err(),
    );
    same_error(
        "metadata",
        &lfs::metadata(&path).unwrap_err(),
        &std::fs::metadata(&path).unwrap_err(),
    );
    same_error(
        "rename target",
        &lfs::rename(dir.path(), &path).unwrap_err(),
        &std::fs::rename(dir.path(), &path).unwrap_err(),
    );
    assert_eq!(
        lfs::metadata(&path).unwrap_err().kind(),
        lio::ErrorKind::InvalidInput
    );
}

#[test]
fn unpaired_surrogates_survive_round_trips() {
    let dir = TempDir::new("wtf8");
    // `a`, a lone high surrogate, `b`, a lone low surrogate, and a pair.
    let wide = [0x61, 0xD800, 0x62, 0xDC00, 0xD83D, 0xDE00];
    let lite_name = litestd::ffi::OsString::from_wide(&wide);
    let lite_path = litestd::path::Path::new(&dir.path()).join(&lite_name);
    let std_path = dir.std_path().join(std::ffi::OsString::from_wide(&wide));
    // The conversion is lossless: both reach the same file, or fail alike
    // on file systems that cannot store the name.
    let created = same(
        "create",
        lfs::write(&lite_path, b"surrogates"),
        std::fs::write(&std_path, b"surrogates"),
    );
    if created.is_none() {
        // Wine stores names in UTF-8, which has no lone surrogates.
        return;
    }
    assert_eq!(std::fs::read(&std_path).unwrap(), b"surrogates");
    let std_names: Vec<Vec<u16>> = std::fs::read_dir(dir.std_path())
        .unwrap()
        .map(|e| e.unwrap().file_name().encode_wide().collect())
        .collect();
    assert_eq!(std_names, [wide.to_vec()]);
    let lite_names: Vec<Vec<u16>> = lfs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().encode_wide().collect())
        .collect();
    assert_eq!(lite_names, [wide.to_vec()]);
    lfs::remove_file(&lite_path).unwrap();
}

#[test]
fn supplementary_characters_round_trip() {
    let dir = TempDir::new("utf16");
    let name = "caf\u{e9} \u{1f600} \u{4e2d}\u{6587}";
    lfs::write(dir.join(name), b"unicode").unwrap();
    assert_eq!(std::fs::read(dir.join(name)).unwrap(), b"unicode");
    let lite: Vec<_> = lfs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(lite, [name]);
    let (lite, real) = same(
        "canonicalize",
        lfs::canonicalize(dir.join(name)),
        std::fs::canonicalize(dir.join(name)),
    )
    .unwrap();
    assert_eq!(lite.to_str(), real.to_str());
}

#[test]
fn readonly_files_behave_like_std() {
    let dir = TempDir::new("readonly");
    for name in ["lite", "std"] {
        std::fs::write(dir.join(name), b"data").unwrap();
    }
    let mut lite_perm = lfs::metadata(dir.join("lite")).unwrap().permissions();
    lite_perm.set_readonly(true);
    lfs::set_permissions(dir.join("lite"), lite_perm).unwrap();
    let mut std_perm =
        std::fs::metadata(dir.join("std")).unwrap().permissions();
    std_perm.set_readonly(true);
    std::fs::set_permissions(dir.join("std"), std_perm).unwrap();
    for name in ["lite", "std"] {
        let real = std::fs::metadata(dir.join(name)).unwrap();
        assert!(real.permissions().readonly(), "{name}");
        assert_ne!(real.file_attributes() & FILE_ATTRIBUTE_READONLY, 0);
        let lite = lfs::metadata(dir.join(name)).unwrap();
        same_metadata_ext(name, &lite, &real);
    }
    same(
        "write to read-only",
        lfs::OpenOptions::new()
            .write(true)
            .open(dir.join("lite"))
            .map(drop),
        std::fs::OpenOptions::new()
            .write(true)
            .open(dir.join("std"))
            .map(drop),
    );
    // std deletes read-only files, through POSIX deletion.
    same(
        "remove read-only",
        lfs::remove_file(dir.join("lite")),
        std::fs::remove_file(dir.join("std")),
    );
    assert!(!Path::new(&dir.join("lite")).exists());
    // A read-only file through a handle, then writable again.
    let path = dir.join("handle");
    let file = lfs::File::create(&path).unwrap();
    let mut perm = file.metadata().unwrap().permissions();
    perm.set_readonly(true);
    file.set_permissions(perm.clone()).unwrap();
    assert!(std::fs::metadata(&path).unwrap().permissions().readonly());
    perm.set_readonly(false);
    file.set_permissions(perm).unwrap();
    assert!(!std::fs::metadata(&path).unwrap().permissions().readonly());
}

#[test]
fn rename_replaces_like_std() {
    let dir = TempDir::new("rename");
    for side in ["lite", "std"] {
        std::fs::write(dir.join(&format!("{side}-from")), b"from").unwrap();
        std::fs::write(dir.join(&format!("{side}-to")), b"to").unwrap();
        std::fs::create_dir(dir.join(&format!("{side}-dir"))).unwrap();
        std::fs::write(dir.join(&format!("{side}-dir\\x")), b"x").unwrap();
    }
    same(
        "replace file",
        lfs::rename(dir.join("lite-from"), dir.join("lite-to")),
        std::fs::rename(dir.join("std-from"), dir.join("std-to")),
    );
    assert_eq!(std::fs::read(dir.join("lite-to")).unwrap(), b"from");
    same(
        "file over non-empty directory",
        lfs::rename(dir.join("lite-to"), dir.join("lite-dir")),
        std::fs::rename(dir.join("std-to"), dir.join("std-dir")),
    );
    // `MoveFileExW` refuses to replace a file with a directory, and std
    // retries with POSIX semantics, which do.
    for side in ["lite", "std"] {
        std::fs::create_dir(dir.join(&format!("{side}-empty"))).unwrap();
        std::fs::write(dir.join(&format!("{side}-file")), b"f").unwrap();
    }
    same(
        "directory over file",
        lfs::rename(dir.join("lite-empty"), dir.join("lite-file")),
        std::fs::rename(dir.join("std-empty"), dir.join("std-file")),
    );
    assert_eq!(
        std::fs::metadata(dir.join("lite-file")).unwrap().is_dir(),
        std::fs::metadata(dir.join("std-file")).unwrap().is_dir()
    );
    same(
        "missing source",
        lfs::rename(dir.join("lite-missing"), dir.join("lite-x")),
        std::fs::rename(dir.join("std-missing"), dir.join("std-x")),
    );
}

#[test]
fn metadata_ext_matches_std_everywhere() {
    let dir = TempDir::new("metaext");
    std::fs::write(dir.join("file"), b"12345").unwrap();
    std::fs::create_dir(dir.join("dir")).unwrap();
    let hidden = dir.join("hidden");
    let file = lfs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .attributes(FILE_ATTRIBUTE_HIDDEN)
        .open(&hidden)
        .unwrap();
    drop(file);
    assert_ne!(
        std::fs::metadata(&hidden).unwrap().file_attributes()
            & FILE_ATTRIBUTE_HIDDEN,
        0
    );
    for name in ["file", "dir", "hidden"] {
        let path = dir.join(name);
        let real = std::fs::metadata(&path).unwrap();
        same_metadata_ext(name, &lfs::metadata(&path).unwrap(), &real);
        same_metadata_ext(name, &lfs::symlink_metadata(&path).unwrap(), &real);
        let lite_file = lfs::OpenOptions::new()
            .access_mode(0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(&path)
            .unwrap();
        same_metadata_ext(name, &lite_file.metadata().unwrap(), &real);
    }
    let mut lite_entries: Vec<_> = lfs::read_dir(dir.path())
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (
                e.file_name().into_string().unwrap(),
                e.metadata().unwrap(),
                e.file_type().unwrap(),
            )
        })
        .collect();
    let mut std_entries: Vec<_> = std::fs::read_dir(dir.std_path())
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (
                e.file_name().into_string().unwrap(),
                e.metadata().unwrap(),
                e.file_type().unwrap(),
            )
        })
        .collect();
    lite_entries.sort_by(|a, b| a.0.cmp(&b.0));
    std_entries.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(lite_entries.len(), std_entries.len());
    for ((ln, lm, lt), (rn, rm, rt)) in lite_entries.iter().zip(&std_entries) {
        assert_eq!(ln, rn);
        same_metadata_ext(ln, lm, rm);
        assert_eq!(
            (lt.is_dir(), lt.is_file(), lt.is_symlink()),
            (rt.is_dir(), rt.is_file(), rt.is_symlink())
        );
    }
}

/// Sets the same times on two files, through litestd and std, and
/// compares what each reads back. Wine keeps no creation time of its own,
/// so the times are compared between the two files, not with the values
/// set.
#[test]
fn file_times_ext_sets_creation_time() {
    let dir = TempDir::new("created");
    let (lite_path, std_path) = (dir.join("lite"), dir.join("std"));
    let lite = lfs::File::create(&lite_path).unwrap();
    let real = std::fs::File::create(&std_path).unwrap();
    let times = [
        Duration::from_secs(1_000_000_000),
        Duration::new(1_100_000_000, 123_456_700),
        Duration::new(1_200_000_000, 99),
    ];
    let lite_time = |d: Duration| litestd::time::UNIX_EPOCH + d;
    let std_time = |d: Duration| std::time::UNIX_EPOCH + d;
    lite.set_times(
        lfs::FileTimes::new()
            .set_created(lite_time(times[0]))
            .set_modified(lite_time(times[1]))
            .set_accessed(lite_time(times[2])),
    )
    .unwrap();
    real.set_times(
        std::fs::FileTimes::new()
            .set_created(std_time(times[0]))
            .set_modified(std_time(times[1]))
            .set_accessed(std_time(times[2])),
    )
    .unwrap();
    let compare = |what: &str| {
        let lite = lfs::metadata(&lite_path).unwrap();
        let real = std::fs::metadata(&std_path).unwrap();
        assert_eq!(lite_ext(&lite)[1..4], std_ext(&real)[1..4], "{what}");
        assert_eq!(
            lite_epoch(lite.modified().unwrap()),
            std_epoch(real.modified().unwrap())
        );
        assert_eq!(
            lite_epoch(lite.created().unwrap()),
            std_epoch(real.created().unwrap())
        );
    };
    compare("after set_times");
    assert_eq!(
        lite_epoch(lfs::metadata(&lite_path).unwrap().modified().unwrap()),
        Ok(times[1])
    );
    // Times before 1970 and sub-100 ns parts round toward the epoch, as
    // std's `FILETIME` arithmetic does.
    let before = Duration::new(86_400, 50);
    lite.set_modified(litestd::time::UNIX_EPOCH - before)
        .unwrap();
    real.set_modified(std::time::UNIX_EPOCH - before).unwrap();
    compare("before 1970");
    // 1601-01-01 is `FILETIME` 0, which `SetFileTime` reads as "unset".
    let to_1601 = Duration::from_secs(11_644_473_600);
    same(
        "time zero",
        lite.set_modified(litestd::time::UNIX_EPOCH - to_1601),
        real.set_modified(std::time::UNIX_EPOCH - to_1601),
    );
    compare("after time zero");
    // Before 1601, which std's `SystemTime` cannot hold.
    let error = lite
        .set_modified(
            litestd::time::UNIX_EPOCH - to_1601 - Duration::from_secs(1),
        )
        .unwrap_err();
    assert_eq!(error.kind(), lio::ErrorKind::InvalidInput);
    // Through paths, following links or not.
    let times = lfs::FileTimes::new().set_modified(lite_time(times[0]));
    lfs::set_times(&lite_path, times).unwrap();
    lfs::set_times_nofollow(&lite_path, times).unwrap();
    assert_eq!(
        lite_epoch(lfs::metadata(&lite_path).unwrap().modified().unwrap()),
        Ok(Duration::from_secs(1_000_000_000))
    );
}

#[test]
fn seek_read_and_write_match_std() {
    let dir = TempDir::new("seek");
    let lite = lfs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("lite"))
        .unwrap();
    let real = std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("std"))
        .unwrap();
    assert_eq!(
        lite.seek_write(b"hello", 10).unwrap(),
        real.seek_write(b"hello", 10).unwrap()
    );
    let mut lite_buf = [0u8; 20];
    let mut std_buf = [0u8; 20];
    assert_eq!(
        lite.seek_read(&mut lite_buf, 8).unwrap(),
        real.seek_read(&mut std_buf, 8).unwrap()
    );
    assert_eq!(lite_buf, std_buf);
    // Beyond the end: 0 bytes.
    assert_eq!(lite.seek_read(&mut lite_buf, 100).unwrap(), 0);
    assert_eq!(real.seek_read(&mut std_buf, 100).unwrap(), 0);
    // The cursor moves to the end of the transfer, on both.
    let mut lite_rest = Vec::new();
    let mut std_rest = Vec::new();
    lite.seek_read(&mut lite_buf[..2], 10).unwrap();
    real.seek_read(&mut std_buf[..2], 10).unwrap();
    (&lite).read_to_end(&mut lite_rest).unwrap();
    (&real).read_to_end(&mut std_rest).unwrap();
    assert_eq!(lite_rest, std_rest);
    assert_eq!(
        std::fs::read(dir.join("lite")).unwrap(),
        std::fs::read(dir.join("std")).unwrap()
    );
}

#[test]
fn open_options_ext_matches_std() {
    let dir = TempDir::new("optext");
    // No sharing: a second open fails with a sharing violation.
    for (name, lite) in [("lite", true), ("std", false)] {
        let path = dir.join(name);
        let _first = if lite {
            Box::new(
                lfs::File::options()
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .share_mode(0)
                    .open(&path)
                    .unwrap(),
            ) as Box<dyn core::any::Any>
        } else {
            Box::new(
                std::fs::File::options()
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .share_mode(0)
                    .open(&path)
                    .unwrap(),
            )
        };
        let second = std::fs::File::open(&path).unwrap_err();
        assert_eq!(
            second.raw_os_error(),
            Some(ERROR_SHARING_VIOLATION),
            "{name}"
        );
        let second = lfs::File::open(&path).unwrap_err();
        assert_eq!(
            second.raw_os_error(),
            Some(ERROR_SHARING_VIOLATION),
            "{name}"
        );
    }
    // Delete on close.
    let temp = dir.join("temp");
    let file = lfs::File::options()
        .write(true)
        .create_new(true)
        .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
        .open(&temp)
        .unwrap();
    assert!(Path::new(&temp).exists());
    drop(file);
    assert!(!Path::new(&temp).exists());
    // No access at all still reads metadata.
    std::fs::write(dir.join("plain"), b"abc").unwrap();
    let file = lfs::File::options()
        .access_mode(0)
        .open(dir.join("plain"))
        .unwrap();
    assert_eq!(file.metadata().unwrap().len(), 3);
    let mut buf = [0; 3];
    same_error(
        "read without access",
        &(&file).read(&mut buf).unwrap_err(),
        &(&std::fs::File::options()
            .access_mode(0)
            .open(dir.join("plain"))
            .unwrap())
            .read(&mut buf)
            .unwrap_err(),
    );
    // Security QoS flags are accepted for plain files.
    lfs::File::options()
        .read(true)
        .security_qos_flags(SECURITY_IDENTIFICATION)
        .open(dir.join("plain"))
        .unwrap();
}

/// Checks the result of `unlock`. Wine fails the second unlock that std
/// and litestd make after one lock with `ERROR_LOCK_VIOLATION` instead of
/// `ERROR_NOT_LOCKED`, which fails std's `unlock` as well; the lock is gone
/// either way. With nothing locked, Windows fails the first unlock with
/// `ERROR_NOT_LOCKED`, as in std.
#[track_caller]
fn unlocked(result: lio::Result<()>) {
    if let Err(e) = result {
        assert!(unlock_failure(e.raw_os_error()), "{e}");
    }
}

#[track_caller]
fn std_unlocked(result: std::io::Result<()>) {
    if let Err(e) = result {
        assert!(unlock_failure(e.raw_os_error()), "{e}");
    }
}

/// Whether `code` is one that `unlocked` accepts.
const fn unlock_failure(code: Option<i32>) -> bool {
    matches!(code, Some(ERROR_LOCK_VIOLATION | ERROR_NOT_LOCKED))
}

#[test]
fn unlock_matches_std() {
    let dir = TempDir::new("unlock");
    let path = dir.join("file");
    std::fs::write(&path, b"lock").unwrap();
    let lite = lfs::File::options().write(true).open(&path).unwrap();
    let real = std::fs::File::options()
        .write(true)
        .open(dir.join("file"))
        .unwrap();
    // Not locked at all.
    same("unlock unlocked", lite.unlock(), real.unlock());
    lite.lock().unwrap();
    let lite_result = lite.unlock();
    real.lock().unwrap();
    same("unlock locked", lite_result, real.unlock());
    // Both an exclusive and a shared lock on one handle.
    lite.lock().unwrap();
    lite.lock_shared().unwrap();
    let lite_result = lite.unlock();
    real.lock().unwrap();
    real.lock_shared().unwrap();
    same("unlock both", lite_result, real.unlock());
}

#[test]
fn locks_interact_with_std_locks() {
    let dir = TempDir::new("locks");
    let path = dir.join("file");
    std::fs::write(&path, b"lock").unwrap();
    let lite = lfs::File::options()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let real = std::fs::File::options()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    lite.lock().unwrap();
    assert!(matches!(
        real.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    assert!(matches!(
        real.try_lock_shared(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    unlocked(lite.unlock());
    real.lock_shared().unwrap();
    lite.try_lock_shared().unwrap();
    assert!(matches!(
        lite.try_lock(),
        Err(lfs::TryLockError::WouldBlock)
    ));
    unlocked(lite.unlock());
    std_unlocked(real.unlock());
    lite.try_lock().unwrap();
    assert!(matches!(
        real.try_lock_shared(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    unlocked(lite.unlock());
    // A second litestd handle to the same file.
    let other = lfs::File::open(&path).unwrap();
    other.lock_shared().unwrap();
    assert!(matches!(
        lite.try_lock(),
        Err(lfs::TryLockError::WouldBlock)
    ));
    let error = match lite.try_lock() {
        Err(lfs::TryLockError::WouldBlock) => {
            lio::Error::from(lfs::TryLockError::WouldBlock)
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(error.kind(), lio::ErrorKind::WouldBlock);
    unlocked(other.unlock());
    lite.try_lock().unwrap();
    unlocked(lite.unlock());
}

#[test]
fn blocking_lock_waits_for_the_holder() {
    let dir = TempDir::new("lockwait");
    let path = dir.join("file");
    std::fs::write(&path, b"lock").unwrap();
    let holder = std::fs::File::options().write(true).open(&path).unwrap();
    holder.lock().unwrap();
    let waiter = lfs::File::options().write(true).open(&path).unwrap();
    let thread = std::thread::spawn(move || {
        waiter.lock().unwrap();
        unlocked(waiter.unlock());
    });
    std::thread::sleep(Duration::from_millis(100));
    assert!(!thread.is_finished());
    std_unlocked(holder.unlock());
    thread.join().unwrap();
}

#[test]
fn overlapped_handles_transfer_and_lock() {
    let dir = TempDir::new("overlapped");
    let path = dir.join("file");
    let file = lfs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .custom_flags(FILE_FLAG_OVERLAPPED)
        .open(&path)
        .unwrap();
    // Overlapped handles have no file pointer; offsets work.
    assert_eq!(file.seek_write(b"overlapped", 0).unwrap(), 10);
    let mut buf = [0; 10];
    assert_eq!(file.seek_read(&mut buf, 0).unwrap(), 10);
    assert_eq!(&buf, b"overlapped");
    assert_eq!(file.seek_read(&mut buf, 50).unwrap(), 0);
    file.lock().unwrap();
    let other = std::fs::File::open(&path).unwrap();
    assert!(matches!(
        other.try_lock_shared(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    unlocked(file.unlock());
    file.try_lock_shared().unwrap();
    unlocked(file.unlock());
    // A handle from elsewhere may be overlapped too.
    let std_file = std::fs::File::options()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    // SAFETY: the handle is owned by `std_file`, which gives it up.
    let foreign =
        unsafe { lfs::File::from_raw_handle(std_file.into_raw_handle()) };
    foreign.lock().unwrap();
    assert!(matches!(
        other.try_lock_shared(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    assert!(matches!(foreign.try_lock_shared(), Ok(())));
    unlocked(foreign.unlock());
    unlocked(foreign.unlock());
    assert_eq!((&foreign).write(b"OVER").unwrap(), 4);
    assert_eq!(std::fs::read(&path).unwrap(), b"OVERlapped");
}

#[test]
fn handles_convert_like_std() {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;

    let dir = TempDir::new("handles");
    let path = dir.join("file");
    lfs::write(&path, b"handles").unwrap();
    let file = lfs::File::open(&path).unwrap();
    let raw = file.as_raw_handle();
    assert_eq!(file.as_handle().as_raw_handle(), raw);
    let owned = OwnedHandle::from(file);
    assert_eq!(owned.as_raw_handle(), raw);
    let clone = owned.try_clone().unwrap();
    assert_ne!(clone.as_raw_handle(), raw);
    let mut file = lfs::File::from(clone);
    let mut contents = String::new();
    file.read_to_string(&mut contents).unwrap();
    assert_eq!(contents, "handles");
    let raw = file.into_raw_handle();
    // SAFETY: `into_raw_handle` gave up the handle.
    let std_file = unsafe { std::fs::File::from_raw_handle(raw) };
    assert_eq!(std_file.metadata().unwrap().len(), 7);
    // Debug output names the handle, as std's does.
    let borrowed = owned.as_handle();
    // SAFETY: `owned` keeps the handle open while the borrow is used.
    let std_borrowed = unsafe {
        std::os::windows::io::BorrowedHandle::borrow_raw(
            borrowed.as_raw_handle(),
        )
    };
    assert_eq!(format!("{borrowed:?}"), format!("{std_borrowed:?}"));
    assert_eq!(
        format!("{owned:?}").replace("OwnedHandle", ""),
        format!("{borrowed:?}").replace("BorrowedHandle", "")
    );
    // Null and invalid sentinels.
    // SAFETY: null and `INVALID_HANDLE_VALUE` are the sentinels themselves.
    let null = unsafe { HandleOrNull::from_raw_handle(core::ptr::null_mut()) };
    let error = OwnedHandle::try_from(null).unwrap_err();
    // SAFETY: as above.
    let std_null = unsafe {
        std::os::windows::io::HandleOrNull::from_raw_handle(
            core::ptr::null_mut(),
        )
    };
    let std_error =
        std::os::windows::io::OwnedHandle::try_from(std_null).unwrap_err();
    assert_eq!(error.to_string(), std_error.to_string());
    // SAFETY: as above.
    let invalid =
        unsafe { HandleOrInvalid::from_raw_handle(INVALID_HANDLE_VALUE) };
    let error = OwnedHandle::try_from(invalid).unwrap_err();
    // SAFETY: as above.
    let std_invalid = unsafe {
        std::os::windows::io::HandleOrInvalid::from_raw_handle(
            INVALID_HANDLE_VALUE,
        )
    };
    let std_error =
        std::os::windows::io::OwnedHandle::try_from(std_invalid).unwrap_err();
    assert_eq!(error.to_string(), std_error.to_string());
    // Valid handles convert.
    let raw = owned.try_clone().unwrap().into_raw_handle();
    // SAFETY: `into_raw_handle` gave up the handle.
    let handle =
        OwnedHandle::try_from(unsafe { HandleOrNull::from_raw_handle(raw) })
            .unwrap();
    drop(handle);
    // A null handle clones to null, as for detached stdio.
    // SAFETY: null is not a handle to anything, so nothing can close it.
    let borrowed_null =
        unsafe { BorrowedHandle::borrow_raw(core::ptr::null_mut()) };
    assert!(
        borrowed_null
            .try_clone_to_owned()
            .unwrap()
            .into_raw_handle()
            .is_null()
    );
}

#[test]
fn file_debug_names_the_path() {
    let dir = TempDir::new("debug");
    let path = dir.join("file");
    let lite = lfs::File::create(&path).unwrap();
    let real = std::fs::File::open(&path).unwrap();
    let path_part =
        |s: String| s.split_once("path: ").map(|(_, p)| p.to_owned());
    assert_eq!(
        path_part(format!("{lite:?}")),
        path_part(format!("{real:?}"))
    );
    assert!(format!("{lite:?}").starts_with("File { handle: 0x"));
}

#[test]
fn links_match_std() {
    let dir = TempDir::new("links");
    std::fs::write(dir.join("target.txt"), b"target").unwrap();
    std::fs::create_dir(dir.join("target_dir")).unwrap();
    std::fs::write(dir.join(r"target_dir\inner"), b"inner").unwrap();
    junction(&dir.join("target_dir"), &dir.join("junction")).unwrap();
    let symlinks = symlink("target.txt", &dir.join("file_link"), false)
        .unwrap()
        && symlink("target_dir", &dir.join("dir_link"), true).unwrap()
        && symlink(&dir.join("target.txt"), &dir.join("absolute_link"), false)
            .unwrap()
        && symlink("missing", &dir.join("dangling"), false).unwrap();
    let mut names = vec!["target.txt", "target_dir", "junction", "missing"];
    if symlinks {
        names.extend(["file_link", "dir_link", "absolute_link", "dangling"]);
    }
    for name in names {
        let path = dir.join(name);
        if let Some((lite, real)) =
            same(name, lfs::metadata(&path), std::fs::metadata(&path))
        {
            same_metadata_ext(name, &lite, &real);
        }
        if let Some((lite, real)) = same(
            name,
            lfs::symlink_metadata(&path),
            std::fs::symlink_metadata(&path),
        ) {
            same_metadata_ext(name, &lite, &real);
        }
        if let Some((lite, real)) =
            same(name, lfs::read_link(&path), std::fs::read_link(&path))
        {
            assert_eq!(lite.to_str(), real.to_str(), "{name}: read_link");
        }
        if let Some((lite, real)) =
            same(name, lfs::canonicalize(&path), std::fs::canonicalize(&path))
        {
            assert_eq!(lite.to_str(), real.to_str(), "{name}: canonicalize");
        }
        if let Some((lite, real)) =
            same(name, lfs::exists(&path), std::fs::exists(&path))
        {
            assert_eq!(lite, real, "{name}: exists");
        }
    }
    // Listing through a junction lists the target.
    let lite: Vec<_> = lfs::read_dir(dir.join("junction"))
        .unwrap()
        .map(|e| e.unwrap().path().to_str().unwrap().to_owned())
        .collect();
    let real: Vec<_> = std::fs::read_dir(dir.join("junction"))
        .unwrap()
        .map(|e| e.unwrap().path().to_str().unwrap().to_owned())
        .collect();
    assert_eq!(lite, real);
    // The entries of the directory, links included, as listed.
    let mut lite: Vec<_> = lfs::read_dir(dir.path())
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            let t = e.file_type().unwrap();
            (
                e.file_name().into_string().unwrap(),
                t.is_dir(),
                t.is_file(),
                t.is_symlink(),
                t.is_symlink_dir(),
            )
        })
        .collect();
    let mut real: Vec<_> = std::fs::read_dir(dir.std_path())
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            let t = e.file_type().unwrap();
            (
                e.file_name().into_string().unwrap(),
                t.is_dir(),
                t.is_file(),
                t.is_symlink(),
                t.is_symlink_dir(),
            )
        })
        .collect();
    lite.sort();
    real.sort();
    assert_eq!(lite, real);
}

#[test]
fn litestd_creates_symlinks_std_reads() {
    let dir = TempDir::new("mklink");
    std::fs::write(dir.join("target.txt"), b"target").unwrap();
    std::fs::create_dir(dir.join("target_dir")).unwrap();
    match litestd::os::windows::fs::symlink_file(
        "target.txt",
        dir.join("file_link"),
    ) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD) => return,
        Err(e) => panic!("symlink_file: {e}"),
    }
    litestd::os::windows::fs::symlink_dir("target_dir", dir.join("dir_link"))
        .unwrap();
    assert_eq!(
        std::fs::read_link(dir.join("file_link")).unwrap(),
        Path::new("target.txt")
    );
    assert_eq!(
        std::fs::read_link(dir.join("dir_link")).unwrap(),
        Path::new("target_dir")
    );
    assert!(
        std::fs::symlink_metadata(dir.join("file_link"))
            .unwrap()
            .file_type()
            .is_symlink_file()
    );
    assert!(
        std::fs::symlink_metadata(dir.join("dir_link"))
            .unwrap()
            .file_type()
            .is_symlink_dir()
    );
    assert_eq!(std::fs::read(dir.join("file_link")).unwrap(), b"target");
    same_error(
        "existing link",
        &litestd::os::windows::fs::symlink_file("x", dir.join("file_link"))
            .unwrap_err(),
        &std::os::windows::fs::symlink_file("x", dir.join("file_link"))
            .unwrap_err(),
    );
}

#[test]
fn hard_links_copies_and_existence_match_std() {
    let dir = TempDir::new("copy");
    std::fs::write(dir.join("source"), b"copy me").unwrap();
    let mut perm = std::fs::metadata(dir.join("source")).unwrap().permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(dir.join("source"), perm).unwrap();
    let (lite, real) = same(
        "copy",
        lfs::copy(dir.join("source"), dir.join("lite")),
        std::fs::copy(dir.join("source"), dir.join("std")),
    )
    .unwrap();
    assert_eq!(lite, real);
    // Copying keeps the attributes, the size and the write time; the other
    // times are those of the copy.
    let lite = lite_ext(&lfs::metadata(dir.join("lite")).unwrap());
    let real = std_ext(&std::fs::metadata(dir.join("std")).unwrap());
    assert_eq!([lite[0], lite[3], lite[4]], [real[0], real[3], real[4]]);
    same(
        "copy over read-only",
        lfs::copy(dir.join("source"), dir.join("lite")),
        std::fs::copy(dir.join("source"), dir.join("std")),
    );
    same(
        "copy a directory",
        lfs::copy(dir.path(), dir.join("lite-dir")),
        std::fs::copy(dir.path(), dir.join("std-dir")),
    );
    same(
        "hard link",
        lfs::hard_link(dir.join("source"), dir.join("lite-link")),
        std::fs::hard_link(dir.join("source"), dir.join("std-link")),
    );
    same(
        "hard link exists",
        lfs::hard_link(dir.join("source"), dir.join("lite-link")),
        std::fs::hard_link(dir.join("source"), dir.join("std-link")),
    );
    for name in [
        "source",
        "lite-link",
        "missing",
        r"missing\deeper",
        r"source\inside",
    ] {
        let path = dir.join(name);
        let (lite, real) =
            same(name, lfs::exists(&path), std::fs::exists(&path)).unwrap();
        assert_eq!(lite, real, "{name}");
        assert_eq!(
            Path::new(&path).exists(),
            litestd::path::Path::new(&path).exists()
        );
    }
}

#[test]
fn directory_errors_match_std() {
    let dir = TempDir::new("direrr");
    for side in ["lite", "std"] {
        std::fs::create_dir(dir.join(side)).unwrap();
        std::fs::write(dir.join(&format!(r"{side}\file")), b"").unwrap();
        std::fs::create_dir(dir.join(&format!(r"{side}\full"))).unwrap();
        std::fs::write(dir.join(&format!(r"{side}\full\x")), b"").unwrap();
    }
    for name in ["file", "missing", "full", r"missing\sub", r"file\sub"] {
        let lite = dir.join(&format!(r"lite\{name}"));
        let real = dir.join(&format!(r"std\{name}"));
        same(
            name,
            lfs::read_dir(&lite).map(drop),
            std::fs::read_dir(&real).map(drop),
        );
        same(name, lfs::remove_dir(&lite), std::fs::remove_dir(&real));
        same(name, lfs::create_dir(&lite), std::fs::create_dir(&real));
        same(name, lfs::remove_file(&lite), std::fs::remove_file(&real));
        same(name, lfs::remove_dir(&lite), std::fs::remove_dir(&real));
    }
    same(
        "read_dir of empty",
        lfs::read_dir("").map(drop),
        std::fs::read_dir("").map(drop),
    );
    same(
        "metadata of empty",
        lfs::metadata("").map(drop),
        std::fs::metadata("").map(drop),
    );
    // An empty directory lists nothing.
    std::fs::create_dir(dir.join("empty")).unwrap();
    assert_eq!(lfs::read_dir(dir.join("empty")).unwrap().count(), 0);
    // Trailing separators and verbatim paths list the same entries, with
    // the same paths, as std.
    let base = dir.join("std");
    for path in [
        base.clone() + r"\",
        base.clone() + "/",
        format!(r"\\?\{base}"),
    ] {
        let mut lite: Vec<_> = lfs::read_dir(&path)
            .unwrap()
            .map(|e| e.unwrap().path().to_str().unwrap().to_owned())
            .collect();
        let mut real: Vec<_> = std::fs::read_dir(&path)
            .unwrap()
            .map(|e| e.unwrap().path().to_str().unwrap().to_owned())
            .collect();
        lite.sort();
        real.sort();
        assert_eq!(lite, real, "{path}");
        assert_eq!(
            format!("{:?}", lfs::read_dir(&path).unwrap()),
            format!("{:?}", std::fs::read_dir(&path).unwrap())
        );
    }
}

#[test]
fn write_and_read_through_std_and_litestd() {
    let dir = TempDir::new("rw");
    let path = dir.join("file");
    let mut lite = lfs::File::create(&path).unwrap();
    lite.write_all(&vec![7; 100_000]).unwrap();
    lite.sync_all().unwrap();
    lite.set_len(50_000).unwrap();
    drop(lite);
    let mut real = std::fs::File::open(&path).unwrap();
    let mut contents = Vec::new();
    real.read_to_end(&mut contents).unwrap();
    assert_eq!(contents, vec![7; 50_000]);
    let mut appended = lfs::File::options().append(true).open(&path).unwrap();
    appended.write_all(b"end").unwrap();
    let lite_all = lfs::read(&path).unwrap();
    assert_eq!(lite_all.len(), 50_003);
    assert!(lite_all.ends_with(b"end"));
    // Create and truncate keeps the attributes, as in std.
    let file = lfs::File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .attributes(FILE_ATTRIBUTE_HIDDEN)
        .open(dir.join("hidden"))
        .unwrap();
    drop(file);
    lfs::File::create(dir.join("hidden")).unwrap();
    std::fs::File::create(dir.join("hidden")).unwrap();
    assert_ne!(
        std::fs::metadata(dir.join("hidden"))
            .unwrap()
            .file_attributes()
            & FILE_ATTRIBUTE_HIDDEN,
        0
    );
}

#[test]
fn handle_types_have_std_auto_traits() {
    use core::panic::{RefUnwindSafe, UnwindSafe};
    const fn all<T: Send + Sync + Unpin + UnwindSafe + RefUnwindSafe>() {}
    all::<OwnedHandle>();
    all::<BorrowedHandle<'static>>();
    all::<HandleOrNull>();
    all::<HandleOrInvalid>();
    all::<litestd::os::windows::io::NullHandleError>();
    all::<litestd::os::windows::io::InvalidHandleError>();
    // The same holds for std's types.
    all::<std::os::windows::io::OwnedHandle>();
    all::<std::os::windows::io::HandleOrNull>();
}

#[test]
fn roots_and_bad_components_match_std() {
    let dir = TempDir::new("roots");
    std::fs::write(dir.join("file"), b"x").unwrap();
    std::fs::create_dir(dir.join("sub")).unwrap();
    let system_drive =
        std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let inputs = [
        format!(r"{system_drive}\"),
        format!(r"\\?\{system_drive}\"),
        dir.join(r"file\sub"),
        dir.join(r"file\"),
        dir.join(r"missing\sub"),
        dir.join("file:stream"),
        dir.join("missing"),
        dir.join("missing."),
        dir.join("file."),
        dir.join("file:missing_stream"),
        dir.join(r"file\missing"),
        format!(r"\\?\{}", dir.join("missing")),
        format!(r"\\?\{}", dir.join(r"file\")),
        "Z:\\no\\such\\drive".to_owned(),
        dir.join(r"file\."),
        dir.path() + r"\.",
        dir.join(r"sub\.."),
    ];
    for path in &inputs {
        let lite = lfs::metadata(path);
        let real = std::fs::metadata(path);
        if let Some((lite, real)) = same(path, lite, real) {
            same_metadata(path, &lite, &real);
            assert_eq!(
                lite.file_attributes(),
                real.file_attributes(),
                "{path}"
            );
            assert_eq!(lite.len(), real.len(), "{path}");
        }
        let lite = lfs::symlink_metadata(path);
        let real = std::fs::symlink_metadata(path);
        if let Some((lite, real)) = same(path, lite, real) {
            assert_eq!(
                lite.file_attributes(),
                real.file_attributes(),
                "{path}"
            );
        }
        if let Some((lite, real)) =
            same(path, lfs::exists(path), std::fs::exists(path))
        {
            assert_eq!(lite, real, "{path}");
        }
    }
}

/// `File::metadata` makes one query where std makes two; on handles to
/// other objects than files, both must still agree.
#[test]
fn metadata_of_pipes_matches_std() {
    let (reader, writer) = std::io::pipe().unwrap();
    let std_reader =
        std::fs::File::from(std::os::windows::io::OwnedHandle::from(reader));
    let raw = std_reader.try_clone().unwrap().into_raw_handle();
    // SAFETY: `into_raw_handle` gave up the handle.
    let lite_reader = unsafe { lfs::File::from_raw_handle(raw) };
    let lite = lite_reader.metadata();
    if let Some((lite, real)) =
        same("pipe metadata", lite, std_reader.metadata())
    {
        assert_eq!(lite_ext(&lite), std_ext(&real));
        let (lt, rt) = (lite.file_type(), real.file_type());
        assert_eq!(
            (lt.is_file(), lt.is_dir(), lt.is_symlink()),
            (rt.is_file(), rt.is_dir(), rt.is_symlink())
        );
    }
    // Reading from a pipe whose writer closed ends the stream, as in std.
    drop(writer);
    let mut buf = [0; 4];
    assert_eq!((&lite_reader).read(&mut buf).unwrap(), 0);
}
