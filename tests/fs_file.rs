//! `litestd::fs::File` and `OpenOptions`, compared with std doing the same
//! operations in the same directory.

// wasm32-unknown-unknown has no file system: `tests/unsupported.rs`
// compares its errors with std's.
#![cfg(all(feature = "fs", not(target_os = "unknown")))]
#![allow(clippy::unwrap_used, reason = "helpers, like tests, fail on errors")]
#![allow(
    clippy::incompatible_msrv,
    reason = "the tests compare with std APIs newer than litestd's MSRV"
)]

extern crate alloc;

mod fs_util;

use alloc::sync::Arc;
use core::{
    panic::{RefUnwindSafe, UnwindSafe},
    time::Duration,
};
use std::{
    io::{Read as _, Seek as _, Write as _},
    sync::{Barrier, mpsc},
    thread,
};

use fs_util::{TempDir, pattern, same, same_error, same_metadata};
use litestd::{
    fs as lfs,
    io::{self as lio, Read as _, Seek as _, Write as _},
};

const fn assert_traits<T: Send + Sync + Unpin + UnwindSafe + RefUnwindSafe>() {}

const fn send_sync<T: Send + Sync + Unpin>() {}

#[test]
fn auto_traits_match_std() {
    assert_traits::<lfs::File>();
    assert_traits::<lfs::Metadata>();
    assert_traits::<lfs::ReadDir>();
    assert_traits::<lfs::DirEntry>();
    assert_traits::<lfs::OpenOptions>();
    assert_traits::<lfs::Permissions>();
    assert_traits::<lfs::FileType>();
    assert_traits::<lfs::FileTimes>();
    assert_traits::<lfs::DirBuilder>();
    send_sync::<lfs::TryLockError>();
}

/// Every combination of the six boolean options, on an existing file and on
/// a missing one: the same result, error and effect on the file as std.
#[test]
fn open_options_match_std() {
    let dir = TempDir::new("options");
    for existing in [false, true] {
        for bits in 0..64u32 {
            let bit = |n: u32| bits & (1 << n) != 0;
            let (lite, real) = (dir.join("lite"), dir.join("std"));
            for path in [&lite, &real] {
                let _ = std::fs::remove_file(path);
                if existing {
                    std::fs::write(path, b"data").unwrap();
                }
            }
            let what = format!("existing {existing}, options {bits:06b}");
            let lite_result = lfs::OpenOptions::new()
                .read(bit(0))
                .write(bit(1))
                .append(bit(2))
                .truncate(bit(3))
                .create(bit(4))
                .create_new(bit(5))
                .open(&lite);
            let std_result = std::fs::OpenOptions::new()
                .read(bit(0))
                .write(bit(1))
                .append(bit(2))
                .truncate(bit(3))
                .create(bit(4))
                .create_new(bit(5))
                .open(&real);
            match (lite_result, std_result) {
                // Rust 1.100 names this conflict, where older std reuses the
                // message for missing write access.
                (Err(a), Err(b)) if bit(2) && bit(3) && !bit(5) => {
                    assert_eq!(
                        format!("{:?}", a.kind()),
                        format!("{:?}", b.kind()),
                        "{what}"
                    );
                    assert_eq!(
                        a.to_string(),
                        "append and truncate cannot both be enabled"
                    );
                }
                (a, b) => drop(same(&what, a, b)),
            }
            assert_eq!(
                std::fs::read(&lite).ok(),
                std::fs::read(&real).ok(),
                "{what}"
            );
        }
    }
}

#[test]
fn open_options_debug_matches_std() {
    let mut lite = lfs::OpenOptions::new();
    let mut real = std::fs::OpenOptions::new();
    assert_eq!(format!("{lite:?}"), format!("{real:?}"));
    lite.read(true).append(true).create_new(true);
    real.read(true).append(true).create_new(true);
    assert_eq!(format!("{lite:?}"), format!("{real:?}"));
    assert_eq!(format!("{:?}", lite.clone()), format!("{real:?}"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;

        use litestd::os::unix::fs::OpenOptionsExt as _;
        lite.mode(0o640).custom_flags(libc::O_NOFOLLOW);
        real.mode(0o640).custom_flags(libc::O_NOFOLLOW);
        assert_eq!(format!("{lite:?}"), format!("{real:?}"));
    }
}

#[test]
fn create_open_and_create_new() {
    let dir = TempDir::new("create");
    let path = dir.join("file");
    let mut file = lfs::File::create(&path).unwrap();
    file.write_all(b"hello").unwrap();
    drop(file);
    let mut text = String::new();
    lfs::File::open(&path)
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "hello");
    same(
        "create_new on an existing file",
        lfs::File::create_new(&path),
        std::fs::File::create_new(&path),
    );
    same(
        "open a missing file",
        lfs::File::open(dir.join("missing")),
        std::fs::File::open(dir.join("missing")),
    );
    same(
        "open a missing directory",
        lfs::File::create(dir.join("missing/file")),
        std::fs::File::create(dir.join("missing/file")),
    );
    let mut new = lfs::File::create_new(dir.join("new")).unwrap();
    new.write_all(b"new").unwrap();
    new.rewind().unwrap();
    let mut back = Vec::new();
    new.read_to_end(&mut back).unwrap();
    assert_eq!(back, b"new");
    let mut options = lfs::File::options();
    options.append(true);
    let mut appender = options.open(&path).unwrap();
    appender.write_all(b" world").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"hello world");
}

#[cfg(unix)]
#[test]
fn create_new_refuses_a_dangling_symlink() {
    let dir = TempDir::new("dangling");
    let link = dir.join("link");
    std::os::unix::fs::symlink(dir.join("nowhere"), &link).unwrap();
    same(
        "create_new over a dangling symlink",
        lfs::File::create_new(&link),
        std::fs::File::create_new(&link),
    );
    assert!(!std::path::Path::new(&dir.join("nowhere")).exists());
}

#[test]
fn read_write_seek_match_std() {
    let dir = TempDir::new("seek");
    let (lite_path, std_path) = (dir.join("lite"), dir.join("std"));
    let mut lite = lfs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lite_path)
        .unwrap();
    let mut real = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&std_path)
        .unwrap();
    let data = pattern(10_000);
    // Miri shortens reads and writes on purpose, so whole buffers go
    // through `write_all` and `read_exact`.
    lite.write_all(&data).unwrap();
    real.write_all(&data).unwrap();
    let seeks = [
        (lio::SeekFrom::Start(10), std::io::SeekFrom::Start(10)),
        (lio::SeekFrom::Current(-3), std::io::SeekFrom::Current(-3)),
        (lio::SeekFrom::End(-100), std::io::SeekFrom::End(-100)),
        (lio::SeekFrom::End(50), std::io::SeekFrom::End(50)),
        (
            lio::SeekFrom::Current(-20_000),
            std::io::SeekFrom::Current(-20_000),
        ),
        (
            lio::SeekFrom::Start(u64::MAX),
            std::io::SeekFrom::Start(u64::MAX),
        ),
        (lio::SeekFrom::Start(4_000), std::io::SeekFrom::Start(4_000)),
    ];
    for (a, b) in seeks {
        let what = format!("{b:?}");
        match (lite.seek(a), real.seek(b)) {
            (Ok(a), Ok(b)) => assert_eq!(a, b, "{what}"),
            // Rust 1.100 classifies Windows' `ERROR_NEGATIVE_SEEK` as
            // `InvalidInput`, where older std leaves it uncategorized.
            (Err(a), Err(b)) => {
                assert_eq!(a.raw_os_error(), b.raw_os_error(), "{what}");
                assert_eq!(a.to_string(), b.to_string(), "{what}");
            }
            (a, b) => panic!("{what}: {a:?} vs {b:?}"),
        }
        assert_eq!(
            lite.stream_position().unwrap(),
            real.stream_position().unwrap()
        );
    }
    let (mut a, mut b) = ([0; 700], [0; 700]);
    lite.read_exact(&mut a).unwrap();
    real.read_exact(&mut b).unwrap();
    assert_eq!(a, b);
    lite.seek_relative(-5).unwrap();
    real.seek_relative(-5).unwrap();
    assert_eq!(
        lite.stream_position().unwrap(),
        real.stream_position().unwrap()
    );
    lite.rewind().unwrap();
    let mut all = Vec::new();
    lite.read_to_end(&mut all).unwrap();
    assert_eq!(all, data);
    assert_eq!(lite.read(&mut a).unwrap(), 0);
}

#[test]
fn vectored_io() {
    let dir = TempDir::new("vectored");
    let path = dir.join("file");
    let mut file = lfs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&path)
        .unwrap();
    let bufs = [
        lio::IoSlice::new(b"abc"),
        lio::IoSlice::new(b""),
        lio::IoSlice::new(b"defg"),
    ];
    // Miri shortens writes and reads on purpose; the rest follows.
    let n = file.write_vectored(&bufs).unwrap();
    assert!((1..=7).contains(&n));
    file.write_all(&b"abcdefg"[n..]).unwrap();
    file.rewind().unwrap();
    let (mut a, mut b) = ([0; 2], [0; 10]);
    let mut bufs = [lio::IoSliceMut::new(&mut a), lio::IoSliceMut::new(&mut b)];
    let n = file.read_vectored(&mut bufs).unwrap();
    assert!((1..=7).contains(&n));
    let mut rest = Vec::new();
    file.read_to_end(&mut rest).unwrap();
    let whole: Vec<u8> = a[..n.min(2)]
        .iter()
        .chain(&b[..n.saturating_sub(2)])
        .chain(&rest)
        .copied()
        .collect();
    assert_eq!(whole, b"abcdefg");
    let many: Vec<_> = (0..2000).map(|_| lio::IoSlice::new(b"x")).collect();
    // One call writes at most `IOV_MAX` buffers.
    let n = file.write_vectored(&many).unwrap();
    assert!((1..=2000).contains(&n));
}

#[test]
fn read_to_end_reads_what_is_left() {
    let dir = TempDir::new("read-to-end");
    let path = dir.join("file");
    let data = pattern(if cfg!(miri) { 20_000 } else { 100_000 });
    std::fs::write(&path, &data).unwrap();
    let mut file = lfs::File::open(&path).unwrap();
    file.seek(lio::SeekFrom::Start(12_345)).unwrap();
    let mut buf = b"prefix".to_vec();
    assert_eq!(file.read_to_end(&mut buf).unwrap(), data.len() - 12_345);
    assert_eq!(&buf[..6], b"prefix");
    assert_eq!(&buf[6..], &data[12_345..]);
    // At the end, nothing is left and nothing is appended.
    assert_eq!(file.read_to_end(&mut buf).unwrap(), 0);
    // `&File` reads too, sharing the cursor.
    file.rewind().unwrap();
    let mut again = Vec::new();
    (&file).read_to_end(&mut again).unwrap();
    assert_eq!(again, data);
}

#[test]
fn read_to_string_rejects_invalid_utf8_like_std() {
    let dir = TempDir::new("utf8");
    let path = dir.join("file");
    std::fs::write(&path, b"valid \xF0\x9F\x92\x96 then \xFF invalid").unwrap();
    let mut lite = String::from("kept");
    let mut real = String::from("kept");
    same(
        "read_to_string",
        lfs::File::open(&path).unwrap().read_to_string(&mut lite),
        std::fs::File::open(&path)
            .unwrap()
            .read_to_string(&mut real),
    );
    assert_eq!(lite, "kept");
    same(
        "fs::read_to_string",
        lfs::read_to_string(&path),
        std::fs::read_to_string(&path),
    );
    std::fs::write(&path, "caf\u{e9} \u{1F496}").unwrap();
    assert_eq!(lfs::read_to_string(&path).unwrap(), "caf\u{e9} \u{1F496}");
}

#[test]
fn read_and_write_whole_files_match_std() {
    let dir = TempDir::new("whole");
    let lens: &[usize] = if cfg!(miri) {
        &[0, 33, 8193]
    } else {
        &[0, 1, 31, 32, 33, 4096, 8191, 8192, 8193, 1 << 20]
    };
    for &len in lens {
        let path = dir.join(&format!("file-{len}"));
        let data = pattern(len);
        lfs::write(&path, &data).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), data, "len {len}");
        assert_eq!(lfs::read(&path).unwrap(), data, "len {len}");
        let text: String =
            data.iter().map(|b| char::from(b'a' + b % 26)).collect();
        lfs::write(&path, &text).unwrap();
        assert_eq!(lfs::read_to_string(&path).unwrap(), text, "len {len}");
    }
    same(
        "read a directory",
        lfs::read(dir.path()),
        std::fs::read(dir.path()),
    );
    same(
        "read a missing file",
        lfs::read(dir.join("missing")),
        std::fs::read(dir.join("missing")),
    );
}

/// Files in `/proc` report a size of zero but have contents.
#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(miri, ignore = "Miri warns about files in /proc")]
fn read_proc_files() {
    let lite = lfs::read_to_string("/proc/version").unwrap();
    let real = std::fs::read_to_string("/proc/version").unwrap();
    assert_ne!(lite, "");
    assert_eq!(lite, real);
    assert_eq!(lfs::metadata("/proc/version").unwrap().len(), 0);
}

#[test]
fn set_len_matches_std() {
    let dir = TempDir::new("set-len");
    let (lite_path, std_path) = (dir.join("lite"), dir.join("std"));
    let lite = lfs::File::create(&lite_path).unwrap();
    let real = std::fs::File::create(&std_path).unwrap();
    for len in [10, if cfg!(miri) { 1 << 14 } else { 1 << 20 }, 3, 0] {
        lite.set_len(len).unwrap();
        real.set_len(len).unwrap();
        assert_eq!(std::fs::metadata(&lite_path).unwrap().len(), len);
        assert_eq!(
            std::fs::read(&lite_path).unwrap(),
            std::fs::read(&std_path).unwrap()
        );
    }
    for len in [u64::MAX, 1 << 63] {
        let (a, b) = (lite.set_len(len), real.set_len(len));
        // On Unix and WASI, Rust 1.100 words the conversion error more
        // precisely than older std; litestd follows it. Windows hands the
        // length to the OS.
        #[cfg(any(unix, target_os = "wasi"))]
        {
            let (a, b) = (a.unwrap_err(), b.unwrap_err());
            assert_eq!(format!("{:?}", a.kind()), format!("{:?}", b.kind()));
            assert_eq!(a.to_string(), "number too large to fit in target type");
        }
        #[cfg(not(any(unix, target_os = "wasi")))]
        same("set_len out of range", a, b);
    }
    let read_only = lfs::File::open(&lite_path).unwrap();
    let std_read_only = std::fs::File::open(&std_path).unwrap();
    same(
        "set_len read-only",
        read_only.set_len(1),
        std_read_only.set_len(1),
    );
}

/// An `Arc<File>` reads, writes and seeks through the shared file, with the
/// same results as std's `Arc<File>` doing the same to its own file.
#[test]
fn shared_files_match_std() {
    let dir = TempDir::new("arc");
    let lite_file = lfs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(dir.join("lite"))
        .unwrap();
    let std_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(dir.join("std"))
        .unwrap();
    let (mut lite, mut real) = (Arc::new(lite_file), Arc::new(std_file));
    let (mut lite2, mut real2) = (Arc::clone(&lite), Arc::clone(&real));
    lite.write_all(b"shared data").unwrap();
    real.write_all(b"shared data").unwrap();
    write!(lite, " {}", 42).unwrap();
    write!(real, " {}", 42).unwrap();
    let n = lite.write_vectored(&[lio::IoSlice::new(b"!")]).unwrap();
    assert_eq!(
        n,
        real.write_vectored(&[std::io::IoSlice::new(b"!")]).unwrap()
    );
    lite.flush().unwrap();
    real.flush().unwrap();
    // The clones share the cursor.
    assert_eq!(
        lite2.stream_position().unwrap(),
        real2.stream_position().unwrap()
    );
    lite2.rewind().unwrap();
    real2.rewind().unwrap();
    let (mut a, mut b) = (String::new(), String::new());
    assert_eq!(
        lite2.read_to_string(&mut a).unwrap(),
        real2.read_to_string(&mut b).unwrap()
    );
    assert_eq!(a, b);
    assert_eq!(
        lite.seek(lio::SeekFrom::Start(3)).unwrap(),
        real.seek(std::io::SeekFrom::Start(3)).unwrap()
    );
    lite2.seek_relative(2).unwrap();
    real2.seek_relative(2).unwrap();
    let (mut a, mut b) = ([0; 4], [0; 4]);
    lite.read_exact(&mut a).unwrap();
    real.read_exact(&mut b).unwrap();
    assert_eq!(a, b);
    let (mut a, mut b) = (b"kept".to_vec(), b"kept".to_vec());
    assert_eq!(
        lite.read_to_end(&mut a).unwrap(),
        real.read_to_end(&mut b).unwrap()
    );
    assert_eq!(a, b);
    let (mut a, mut b) = ([0; 1], [0; 1]);
    let mut bufs_a = [lio::IoSliceMut::new(&mut a)];
    let mut bufs_b = [std::io::IoSliceMut::new(&mut b)];
    assert_eq!(
        lite.read_vectored(&mut bufs_a).unwrap(),
        real.read_vectored(&mut bufs_b).unwrap()
    );
    assert_eq!(
        std::fs::read(dir.join("lite")).unwrap(),
        std::fs::read(dir.join("std")).unwrap()
    );
}

#[test]
#[cfg_attr(target_os = "wasi", ignore = "WASI cannot duplicate descriptors")]
fn sync_and_clone() {
    let dir = TempDir::new("sync");
    let mut file = lfs::File::create(dir.join("file")).unwrap();
    file.write_all(b"0123456789").unwrap();
    file.sync_all().unwrap();
    file.sync_data().unwrap();
    file.flush().unwrap();
    let mut clone = file.try_clone().unwrap();
    // Clones share the cursor.
    clone.seek(lio::SeekFrom::Start(4)).unwrap();
    assert_eq!(file.stream_position().unwrap(), 4);
    drop(file);
    clone.write_all(b"xy").unwrap();
    drop(clone);
    assert_eq!(std::fs::read(dir.join("file")).unwrap(), b"0123xy6789");
}

#[test]
fn file_metadata_matches_std() {
    let dir = TempDir::new("metadata");
    let path = dir.join("file");
    std::fs::write(&path, pattern(12_345)).unwrap();
    let lite = lfs::File::open(&path).unwrap().metadata().unwrap();
    let real = std::fs::File::open(&path).unwrap().metadata().unwrap();
    same_metadata("File::metadata", &lite, &real);
    let lite = lfs::metadata(&path).unwrap();
    let real = std::fs::metadata(&path).unwrap();
    same_metadata("fs::metadata", &lite, &real);
    let lite = lfs::metadata(dir.path()).unwrap();
    let real = std::fs::metadata(dir.path()).unwrap();
    same_metadata("directory", &lite, &real);
    assert!(lite.is_dir() && !lite.is_file() && !lite.is_symlink());
    same(
        "missing",
        lfs::metadata(dir.join("missing")),
        std::fs::metadata(dir.join("missing")),
    );
    same(
        "through a file",
        lfs::metadata(dir.join("file/x")),
        std::fs::metadata(dir.join("file/x")),
    );
}

#[test]
fn debug_formats_match_std() {
    let dir = TempDir::new("debug");
    let path = dir.join("file");
    std::fs::write(&path, b"x").unwrap();
    let lite = lfs::metadata(&path).unwrap();
    let real = std::fs::metadata(&path).unwrap();
    // On Windows, litestd's `SystemTime` prints seconds and nanoseconds
    // where std prints 100-nanosecond intervals.
    #[cfg(unix)]
    assert_eq!(format!("{lite:?}"), fs_util::std_debug(&lite, &real));
    assert_eq!(
        format!("{:?}", lite.file_type()),
        format!("{:?}", real.file_type())
    );
    assert_eq!(
        format!("{:?}", lite.permissions()),
        format!("{:?}", real.permissions())
    );
    #[cfg(unix)]
    {
        let lite = lfs::metadata(dir.path()).unwrap();
        let real = std::fs::metadata(dir.path()).unwrap();
        assert_eq!(format!("{lite:?}"), fs_util::std_debug(&lite, &real));
    }
    assert_eq!(
        format!("{:?}", lfs::DirBuilder::new()),
        format!("{:?}", std::fs::DirBuilder::new())
    );
    assert_eq!(
        format!("{:?}", lfs::FileTimes::new()),
        format!("{:?}", std::fs::FileTimes::new())
    );
    #[cfg(unix)]
    {
        let t = litestd::time::UNIX_EPOCH + Duration::new(1_234, 5_678);
        let st = std::time::UNIX_EPOCH + Duration::new(1_234, 5_678);
        assert_eq!(
            format!("{:?}", lfs::FileTimes::new().set_accessed(t)),
            format!("{:?}", std::fs::FileTimes::new().set_accessed(st))
        );
    }
}

#[cfg(unix)]
#[test]
fn mode_debug_matches_std() {
    use std::os::unix::fs::PermissionsExt as _;

    use litestd::os::unix::fs::PermissionsExt as _;
    let modes = [
        0o100_644, 0o104_755, 0o102_750, 0o106_644, 0o041_777, 0o041_776,
        0o040_755, 0o120_777, 0o020_666, 0o060_660, 0o010_600, 0o140_755,
        0o007_777, 0o000_666, 0o101_777, 0o177_777,
    ];
    for mode in modes {
        assert_eq!(
            format!("{:?}", lfs::Permissions::from_mode(mode)),
            format!("{:?}", std::fs::Permissions::from_mode(mode)),
            "{mode:o}"
        );
    }
}

#[test]
#[cfg_attr(miri, ignore = "Miri's descriptors differ from those /proc lists")]
fn file_debug_matches_std() {
    let dir = TempDir::new("file-debug");
    let path = dir.join("file");
    std::fs::write(&path, b"x").unwrap();
    let lite = format!("{:?}", lfs::File::open(&path).unwrap());
    let real = format!("{:?}", std::fs::File::open(&path).unwrap());
    // The descriptor numbers can differ; the rest cannot.
    let strip = |s: &str| s.split_once(", ").map(|(_, rest)| rest.to_owned());
    assert_eq!(strip(&lite), strip(&real));
    #[cfg(unix)]
    {
        let lite = format!(
            "{:?}",
            lfs::OpenOptions::new().append(true).open(&path).unwrap()
        );
        assert!(lite.ends_with("read: false, write: true }"), "{lite}");
    }
}

#[test]
#[cfg_attr(target_os = "wasi", ignore = "WASI has no permissions")]
fn permissions_match_std() {
    let dir = TempDir::new("permissions");
    let path = dir.join("file");
    std::fs::write(&path, b"x").unwrap();
    let writable = std::fs::metadata(&path).unwrap().permissions();
    let mut perm = lfs::metadata(&path).unwrap().permissions();
    assert!(!perm.readonly());
    perm.set_readonly(true);
    assert!(perm.readonly());
    lfs::set_permissions(&path, perm.clone()).unwrap();
    assert!(std::fs::metadata(&path).unwrap().permissions().readonly());
    assert_eq!(lfs::metadata(&path).unwrap().permissions(), perm);
    perm.set_readonly(false);
    // Through a handle, Windows needs the right to write attributes, which
    // `File::open` does not ask for; std fails alike there.
    same(
        "set_permissions through File::open",
        lfs::File::open(&path)
            .unwrap()
            .set_permissions(perm.clone()),
        std::fs::File::open(&path)
            .unwrap()
            .set_permissions(writable.clone()),
    );
    lfs::set_permissions(&path, perm.clone()).unwrap();
    assert!(!std::fs::metadata(&path).unwrap().permissions().readonly());
    // A handle open for writing changes them everywhere.
    let file = lfs::OpenOptions::new().write(true).open(&path).unwrap();
    perm.set_readonly(true);
    file.set_permissions(perm.clone()).unwrap();
    assert!(std::fs::metadata(&path).unwrap().permissions().readonly());
    perm.set_readonly(false);
    file.set_permissions(perm.clone()).unwrap();
    assert!(!std::fs::metadata(&path).unwrap().permissions().readonly());
    same(
        "set_permissions missing",
        lfs::set_permissions(dir.join("missing"), perm),
        std::fs::set_permissions(dir.join("missing"), writable),
    );
}

/// Nanoseconds that file timestamps hold exactly on every platform: Windows
/// counts 100-nanosecond intervals.
const NANOS_A: u32 = if cfg!(windows) {
    123_456_700
} else {
    123_456_789
};
const NANOS_B: u32 = if cfg!(windows) {
    987_654_300
} else {
    987_654_321
};
const NANOS_C: u32 = if cfg!(windows) { 4_200 } else { 42 };

#[test]
#[cfg_attr(
    all(miri, target_vendor = "apple"),
    ignore = "Miri does not implement fsetattrlist"
)]
#[cfg_attr(target_os = "wasi", ignore = "WASI runtimes may round file times")]
fn set_times_matches_std() {
    let dir = TempDir::new("times");
    let path = dir.join("file");
    std::fs::write(&path, b"x").unwrap();
    let accessed =
        litestd::time::UNIX_EPOCH + Duration::new(1_000_000_000, NANOS_A);
    let modified =
        litestd::time::UNIX_EPOCH + Duration::new(1_100_000_000, NANOS_B);
    let file = lfs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_times(
        lfs::FileTimes::new()
            .set_accessed(accessed)
            .set_modified(modified),
    )
    .unwrap();
    let meta = std::fs::metadata(&path).unwrap();
    assert_eq!(
        fs_util::std_epoch(meta.accessed().unwrap()),
        Ok(Duration::new(1_000_000_000, NANOS_A))
    );
    assert_eq!(
        fs_util::std_epoch(meta.modified().unwrap()),
        Ok(Duration::new(1_100_000_000, NANOS_B))
    );
    // Times left unset stay.
    let later = litestd::time::UNIX_EPOCH + Duration::from_secs(1_200_000_000);
    file.set_modified(later).unwrap();
    let meta = std::fs::metadata(&path).unwrap();
    assert_eq!(
        fs_util::std_epoch(meta.accessed().unwrap()),
        Ok(Duration::new(1_000_000_000, NANOS_A))
    );
    assert_eq!(
        fs_util::std_epoch(meta.modified().unwrap()),
        Ok(Duration::from_secs(1_200_000_000))
    );
    let lite = lfs::metadata(&path).unwrap();
    same_metadata("after set_times", &lite, &meta);
    // Before the epoch. Miri's `futimens` refuses such times.
    #[cfg(all(unix, not(miri)))]
    {
        let early =
            litestd::time::UNIX_EPOCH - Duration::new(1_000, 250_000_000);
        file.set_modified(early).unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(
            fs_util::std_epoch(meta.modified().unwrap()),
            Err(Duration::new(1_000, 250_000_000))
        );
        same_metadata(
            "before the epoch",
            &lfs::metadata(&path).unwrap(),
            &meta,
        );
    }
}

#[test]
#[cfg_attr(miri, ignore = "Miri does not implement utimensat")]
#[cfg_attr(target_os = "wasi", ignore = "WASI runtimes may round file times")]
fn set_times_by_path() {
    let dir = TempDir::new("times-path");
    let path = dir.join("file");
    std::fs::write(&path, b"x").unwrap();
    let t = litestd::time::UNIX_EPOCH + Duration::new(1_300_000_000, NANOS_C);
    lfs::set_times(&path, lfs::FileTimes::new().set_modified(t)).unwrap();
    let meta = std::fs::metadata(&path).unwrap();
    assert_eq!(
        fs_util::std_epoch(meta.modified().unwrap()),
        Ok(Duration::new(1_300_000_000, NANOS_C))
    );
    // std's `fs::set_times` is stable from Rust 1.99 on; its error for a
    // missing file is the one `metadata` reports. With no time to set,
    // Linux succeeds without looking the file up, for std as well.
    same(
        "set_times missing",
        lfs::set_times(
            dir.join("missing"),
            lfs::FileTimes::new().set_modified(t),
        ),
        std::fs::metadata(dir.join("missing")).map(drop),
    );
    #[cfg(unix)]
    {
        let link = dir.join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        let t2 = litestd::time::UNIX_EPOCH + Duration::new(1_400_000_000, 7);
        lfs::set_times_nofollow(&link, lfs::FileTimes::new().set_modified(t2))
            .unwrap();
        let link_meta = std::fs::symlink_metadata(&link).unwrap();
        assert_eq!(
            fs_util::std_epoch(link_meta.modified().unwrap()),
            Ok(Duration::new(1_400_000_000, 7))
        );
        // The target is untouched.
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(
            fs_util::std_epoch(meta.modified().unwrap()),
            Ok(Duration::new(1_300_000_000, NANOS_C))
        );
    }
}

#[test]
fn created_time_matches_std() {
    let dir = TempDir::new("created");
    let path = dir.join("file");
    let before = litestd::time::SystemTime::now();
    std::fs::write(&path, b"x").unwrap();
    let lite = lfs::metadata(&path).unwrap().created();
    let real = std::fs::metadata(&path).unwrap().created();
    let born = lite.as_ref().ok().copied();
    fs_util::same_created("created", lite, real);
    // Where the file system records it, the birth time is now, give or take
    // a tick of the coarse clock that file systems read.
    if let Some(born) = born {
        let earliest = before - Duration::from_secs(1);
        let latest = litestd::time::SystemTime::now();
        assert!(earliest <= born && born <= latest, "{born:?}");
    }
}

/// Whether std's own locks work here: wine's `UnlockFile` rejects the range
/// that std and litestd lock, so their locks cannot be released there.
fn std_locks_work(path: &str) -> bool {
    let file = std::fs::File::open(path).unwrap();
    file.lock().unwrap();
    file.unlock().is_ok()
}

#[test]
#[cfg_attr(target_os = "wasi", ignore = "WASI has no file locks")]
fn locks_exclude_other_handles() {
    let dir = TempDir::new("locks");
    let path = dir.join("file");
    std::fs::write(&path, b"x").unwrap();
    if !std_locks_work(&path) {
        return;
    }
    let a = lfs::File::open(&path).unwrap();
    let b = lfs::File::open(&path).unwrap();
    let real = std::fs::File::open(&path).unwrap();
    a.lock().unwrap();
    assert!(matches!(b.try_lock(), Err(lfs::TryLockError::WouldBlock)));
    assert!(matches!(
        b.try_lock_shared(),
        Err(lfs::TryLockError::WouldBlock)
    ));
    assert!(matches!(
        real.try_lock_shared(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    a.unlock().unwrap();
    b.try_lock_shared().unwrap();
    a.lock_shared().unwrap();
    assert!(matches!(a.try_lock(), Err(lfs::TryLockError::WouldBlock)));
    real.try_lock_shared().unwrap();
    b.unlock().unwrap();
    a.unlock().unwrap();
    real.unlock().unwrap();
    b.try_lock().unwrap();
    assert!(matches!(
        real.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    // Closing the file releases its lock.
    drop(b);
    a.try_lock().unwrap();
}

#[test]
#[cfg_attr(miri, ignore = "slow")]
#[cfg_attr(target_os = "wasi", ignore = "WASI has no file locks")]
fn lock_blocks_until_released() {
    let dir = TempDir::new("lock-wait");
    let path = dir.join("file");
    std::fs::write(&path, b"x").unwrap();
    if !std_locks_work(&path) {
        return;
    }
    let holder = lfs::File::open(&path).unwrap();
    holder.lock().unwrap();
    let (sender, receiver) = mpsc::channel();
    let barrier = Arc::new(Barrier::new(2));
    let waiter = {
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            let file = lfs::File::open(&path).unwrap();
            barrier.wait();
            file.lock().unwrap();
            sender.send(()).unwrap();
            file.unlock().unwrap();
        })
    };
    barrier.wait();
    assert!(receiver.recv_timeout(Duration::from_millis(100)).is_err());
    holder.unlock().unwrap();
    receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    waiter.join().unwrap();
}

#[test]
fn try_lock_error_matches_std() {
    let lite = lfs::TryLockError::WouldBlock;
    let real = std::fs::TryLockError::WouldBlock;
    assert_eq!(format!("{lite:?}"), format!("{real:?}"));
    assert_eq!(lite.to_string(), real.to_string());
    // Width, fill, alignment and precision apply as to a `str`.
    assert_eq!(format!("{lite:*^70.20}"), format!("{real:*^70.20}"));
    same_error(
        "WouldBlock",
        &lio::Error::from(lite),
        &std::io::Error::from(real),
    );
    let lite = lfs::TryLockError::Error(lio::Error::from_raw_os_error(9));
    let real =
        std::fs::TryLockError::Error(std::io::Error::from_raw_os_error(9));
    assert_eq!(format!("{lite:?}"), format!("{real:?}"));
    assert_eq!(lite.to_string(), real.to_string());
    assert!(core::error::Error::source(&lite).is_none());
    same_error(
        "Error",
        &lio::Error::from(lite),
        &std::io::Error::from(real),
    );
}

/// Every function that takes a path refuses an interior NUL, whether the
/// path is converted on the stack or on the heap, and never truncates it.
#[test]
fn nul_bytes_are_invalid_input() {
    let dir = TempDir::new("nul");
    std::fs::write(dir.join("a"), b"x").unwrap();
    let short = dir.join("a\0b");
    let long = format!("{}{}", dir.join("a\0b"), "c".repeat(500));
    let good = dir.join("a");
    // std reports the same error for every function, which differs between
    // platforms only in its message.
    let reference = std::fs::metadata(&short).unwrap_err();
    assert_eq!(reference.kind(), std::io::ErrorKind::InvalidInput);
    let expect = |what: &str, r: lio::Result<()>| {
        same_error(what, &r.expect_err(what), &reference);
    };
    for bad in [&short, &long] {
        expect("open", lfs::File::open(bad).map(drop));
        expect("create", lfs::File::create(bad).map(drop));
        expect(
            "options",
            lfs::OpenOptions::new().read(true).open(bad).map(drop),
        );
        expect("metadata", lfs::metadata(bad).map(drop));
        expect("symlink_metadata", lfs::symlink_metadata(bad).map(drop));
        expect("remove_file", lfs::remove_file(bad));
        expect("remove_dir", lfs::remove_dir(bad));
        expect("remove_dir_all", lfs::remove_dir_all(bad));
        expect("create_dir", lfs::create_dir(bad));
        expect("create_dir_all", lfs::create_dir_all(bad));
        expect("read_dir", lfs::read_dir(bad).map(drop));
        expect("read_link", lfs::read_link(bad).map(drop));
        expect("canonicalize", lfs::canonicalize(bad).map(drop));
        expect("exists", lfs::exists(bad).map(drop));
        expect("read", lfs::read(bad).map(drop));
        expect("read_to_string", lfs::read_to_string(bad).map(drop));
        expect("write", lfs::write(bad, b"x"));
        expect("rename from", lfs::rename(bad, dir.join("z")));
        expect("rename to", lfs::rename(&good, bad));
        expect("copy from", lfs::copy(bad, dir.join("z")).map(drop));
        expect("copy to", lfs::copy(&good, bad).map(drop));
        expect("hard_link", lfs::hard_link(&good, bad));
        expect(
            "set_permissions",
            lfs::set_permissions(
                bad,
                lfs::metadata(&good).unwrap().permissions(),
            ),
        );
        expect("set_times", lfs::set_times(bad, lfs::FileTimes::new()));
        expect(
            "set_times_nofollow",
            lfs::set_times_nofollow(bad, lfs::FileTimes::new()),
        );
        #[cfg(unix)]
        {
            use litestd::os::unix::fs as unix;
            expect("symlink", unix::symlink(&good, bad));
            expect("chown", unix::chown(bad, None, None));
            expect("lchown", unix::lchown(bad, None, None));
            expect("chroot", unix::chroot(bad));
        }
        assert!(!litestd::path::Path::new(bad).exists());
        assert!(litestd::path::Path::new(bad).try_exists().is_err());
    }
    // Nothing was created or removed under the truncated name.
    assert_eq!(std::fs::read(&good).unwrap(), b"x");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

/// Paths too long for the stack buffer take the heap.
#[test]
fn long_paths() {
    let dir = TempDir::new("long");
    let deep =
        format!("{}/{}/{}", dir.path(), "d".repeat(200), "e".repeat(200));
    assert!(deep.len() > 400);
    lfs::create_dir_all(&deep).unwrap();
    let file = format!("{deep}/{}", "f".repeat(100));
    lfs::write(&file, b"long").unwrap();
    assert_eq!(lfs::read(&file).unwrap(), b"long");
    same_metadata(
        "long path",
        &lfs::metadata(&file).unwrap(),
        &std::fs::metadata(&file).unwrap(),
    );
    let renamed = format!("{deep}/{}", "g".repeat(100));
    lfs::rename(&file, &renamed).unwrap();
    lfs::remove_file(&renamed).unwrap();
    lfs::remove_dir(&deep).unwrap();
    assert!(!std::path::Path::new(&deep).exists());
}

/// Sets up the same tree for litestd and for std, so that an operation
/// that succeeds cannot change what the other one sees.
fn rename_tree(root: &str) {
    std::fs::create_dir(root).unwrap();
    std::fs::write(format!("{root}/b"), b"b").unwrap();
    std::fs::create_dir(format!("{root}/d")).unwrap();
    std::fs::create_dir(format!("{root}/full")).unwrap();
    std::fs::write(format!("{root}/full/x"), b"x").unwrap();
}

/// Lists what a tree holds afterwards, to compare the effects.
fn tree_state(root: &str) -> Vec<(String, bool)> {
    let mut entries: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (
                e.file_name().into_string().unwrap(),
                e.file_type().unwrap().is_dir(),
            )
        })
        .collect();
    entries.sort();
    entries
}

#[test]
fn rename_and_remove_match_std() {
    type Op = (
        &'static str,
        fn(&str) -> lio::Result<()>,
        fn(&str) -> std::io::Result<()>,
    );
    let ops: [Op; 9] = [
        (
            "rename",
            |r| lfs::rename(format!("{r}/b"), format!("{r}/c")),
            |r| std::fs::rename(format!("{r}/b"), format!("{r}/c")),
        ),
        (
            "rename missing",
            |r| lfs::rename(format!("{r}/a"), format!("{r}/c")),
            |r| std::fs::rename(format!("{r}/a"), format!("{r}/c")),
        ),
        (
            "remove_file missing",
            |r| lfs::remove_file(format!("{r}/a")),
            |r| std::fs::remove_file(format!("{r}/a")),
        ),
        (
            "remove_file directory",
            |r| lfs::remove_file(format!("{r}/d")),
            |r| std::fs::remove_file(format!("{r}/d")),
        ),
        (
            "remove_dir file",
            |r| lfs::remove_dir(format!("{r}/b")),
            |r| std::fs::remove_dir(format!("{r}/b")),
        ),
        (
            "remove_dir full",
            |r| lfs::remove_dir(format!("{r}/full")),
            |r| std::fs::remove_dir(format!("{r}/full")),
        ),
        (
            "remove_dir missing",
            |r| lfs::remove_dir(format!("{r}/a")),
            |r| std::fs::remove_dir(format!("{r}/a")),
        ),
        (
            "rename file over directory",
            |r| lfs::rename(format!("{r}/b"), format!("{r}/d")),
            |r| std::fs::rename(format!("{r}/b"), format!("{r}/d")),
        ),
        (
            "rename directory over file",
            |r| lfs::rename(format!("{r}/d"), format!("{r}/b")),
            |r| std::fs::rename(format!("{r}/d"), format!("{r}/b")),
        ),
    ];
    let dir = TempDir::new("rename");
    for (i, (what, lite, real)) in ops.into_iter().enumerate() {
        let (lite_root, std_root) =
            (dir.join(&format!("lite{i}")), dir.join(&format!("std{i}")));
        rename_tree(&lite_root);
        rename_tree(&std_root);
        same(what, lite(&lite_root), real(&std_root));
        assert_eq!(tree_state(&lite_root), tree_state(&std_root), "{what}");
    }
}

#[test]
fn hard_links_and_exists() {
    let dir = TempDir::new("links");
    std::fs::write(dir.join("a"), b"a").unwrap();
    lfs::hard_link(dir.join("a"), dir.join("b")).unwrap();
    assert_eq!(std::fs::read(dir.join("b")).unwrap(), b"a");
    same(
        "hard_link exists",
        lfs::hard_link(dir.join("a"), dir.join("b")),
        std::fs::hard_link(dir.join("a"), dir.join("b")),
    );
    same(
        "hard_link missing",
        lfs::hard_link(dir.join("x"), dir.join("y")),
        std::fs::hard_link(dir.join("x"), dir.join("y")),
    );
    assert!(lfs::exists(dir.join("a")).unwrap());
    assert!(!lfs::exists(dir.join("x")).unwrap());
    same(
        "exists through a file",
        lfs::exists(dir.join("a/x")),
        std::fs::exists(dir.join("a/x")),
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        assert_eq!(std::fs::metadata(dir.join("a")).unwrap().nlink(), 2);
    }
}
