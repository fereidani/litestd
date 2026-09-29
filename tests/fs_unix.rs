//! `litestd::os::unix::fs` against `std::os::unix::fs`: the extension traits,
//! symlinks, ownership, and the effect of the umask.

#![cfg(all(unix, feature = "fs"))]
#![allow(clippy::unwrap_used, reason = "helpers, like tests, fail on errors")]

mod fs_util;

use std::{
    ffi::CString,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
};

use fs_util::{TempDir, is_root, same, same_metadata};
use litestd::{
    fs as lfs,
    io::Read as _,
    os::unix::fs::{
        DirBuilderExt as _, FileExt as _, FileTypeExt as _,
        OpenOptionsExt as _, PermissionsExt as _,
    },
};

/// The process umask, read without changing it for other threads for more
/// than an instant: every test that depends on it only reads it.
fn umask() -> u32 {
    let path = std::env::temp_dir()
        .join(format!("litestd-fs-umask-{}", std::process::id()));
    let _ = std::fs::remove_dir(&path);
    std::fs::DirBuilder::new().create(&path).unwrap();
    let mode = std::fs::metadata(&path).unwrap().mode() & 0o777;
    std::fs::remove_dir(&path).unwrap();
    !mode & 0o777
}

#[test]
fn new_files_and_directories_follow_the_umask() {
    let dir = TempDir::new("umask");
    let mask = umask();
    lfs::File::create(dir.join("default")).unwrap();
    std::fs::File::create(dir.join("std")).unwrap();
    let lite = std::fs::metadata(dir.join("default")).unwrap().mode();
    assert_eq!(lite, std::fs::metadata(dir.join("std")).unwrap().mode());
    assert_eq!(lite & 0o777, 0o666 & !mask);
    lfs::OpenOptions::new()
        .write(true)
        .create(true)
        .mode(0o751)
        .open(dir.join("mode"))
        .unwrap();
    assert_eq!(
        std::fs::metadata(dir.join("mode")).unwrap().mode() & 0o777,
        0o751 & !mask
    );
    lfs::create_dir(dir.join("d")).unwrap();
    assert_eq!(
        std::fs::metadata(dir.join("d")).unwrap().mode() & 0o777,
        0o777 & !mask
    );
    lfs::DirBuilder::new()
        .mode(0o705)
        .recursive(true)
        .create(dir.join("e/f"))
        .unwrap();
    for name in ["e", "e/f"] {
        assert_eq!(
            std::fs::metadata(dir.join(name)).unwrap().mode() & 0o777,
            0o705 & !mask
        );
    }
}

#[test]
fn permissions_ext_matches_std() {
    for mode in [0o100_644, 0o040_755, 0o4755, 0, 0o177_777] {
        let lite = lfs::Permissions::from_mode(mode);
        let real = std::fs::Permissions::from_mode(mode);
        assert_eq!(lite.mode(), real.mode());
        assert_eq!(lite.readonly(), real.readonly());
        let mut lite = lite;
        lite.set_mode(0o600);
        assert_eq!(lite.mode(), 0o600);
        assert_eq!(lite, lfs::Permissions::from_mode(0o600));
    }
    let dir = TempDir::new("perm-ext");
    let path = dir.join("file");
    std::fs::write(&path, b"x").unwrap();
    lfs::set_permissions(&path, lfs::Permissions::from_mode(0o6750)).unwrap();
    let mode = std::fs::metadata(&path).unwrap().mode();
    assert_eq!(mode, 0o106_750);
    assert_eq!(lfs::metadata(&path).unwrap().permissions().mode(), mode);
}

#[test]
fn custom_flags_match_std() {
    let dir = TempDir::new("custom-flags");
    std::fs::write(dir.join("file"), b"x").unwrap();
    std::os::unix::fs::symlink("file", dir.join("link")).unwrap();
    let lite = lfs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join("link"));
    let real = std::os::unix::fs::OpenOptionsExt::custom_flags(
        std::fs::OpenOptions::new().read(true),
        libc::O_NOFOLLOW,
    )
    .open(dir.join("link"));
    same("O_NOFOLLOW on a symlink", lite, real);
    // Access mode bits in the custom flags are ignored.
    let mut file = lfs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_WRONLY)
        .open(dir.join("file"))
        .unwrap();
    let mut text = String::new();
    file.read_to_string(&mut text).unwrap();
    assert_eq!(text, "x");
}

#[test]
fn file_ext_matches_std() {
    let dir = TempDir::new("file-ext");
    let path = dir.join("file");
    let file = lfs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&path)
        .unwrap();
    // Miri shortens writes and reads on purpose; `write_at` must still
    // write something.
    let n = file.write_at(b"world", 6).unwrap();
    assert!((1..=5).contains(&n));
    file.write_all_at(&b"world"[n..], 6 + n as u64).unwrap();
    file.write_all_at(b"hello ", 0).unwrap();
    let mut buf = [0; 5];
    file.read_exact_at(&mut buf, 6).unwrap();
    assert_eq!(&buf, b"world");
    file.read_exact_at(&mut buf, 0).unwrap();
    assert_eq!(&buf, b"hello");
    // Offsets leave the cursor alone.
    let mut all = String::new();
    (&file).read_to_string(&mut all).unwrap();
    assert_eq!(all, "hello world");
    let real = std::fs::File::open(&path).unwrap();
    let mut buf2 = [0; 5];
    same(
        "read_exact_at past the end",
        file.read_exact_at(&mut buf, 8),
        std::os::unix::fs::FileExt::read_exact_at(&real, &mut buf2, 8),
    );
    same(
        "read_at a huge offset",
        file.read_at(&mut buf, u64::MAX),
        std::os::unix::fs::FileExt::read_at(&real, &mut buf2, u64::MAX),
    );
    same(
        "write_at read-only",
        lfs::File::open(&path).unwrap().write_at(b"x", 0),
        std::os::unix::fs::FileExt::write_at(&real, b"x", 0),
    );
}

#[test]
#[cfg_attr(miri, ignore = "Miri cannot create device nodes or sockets")]
fn file_type_ext_matches_std() {
    use std::os::unix::fs::FileTypeExt as _;
    let dir = TempDir::new("file-type");
    let fifo = dir.join("fifo");
    let c_fifo = CString::new(fifo.as_str()).unwrap();
    // SAFETY: `c_fifo` is a valid C string.
    assert_eq!(unsafe { libc::mkfifo(c_fifo.as_ptr(), 0o600) }, 0);
    let socket = dir.join("socket");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    std::fs::write(dir.join("file"), b"").unwrap();
    std::os::unix::fs::symlink("file", dir.join("link")).unwrap();
    let paths = [
        "/dev/null".to_owned(),
        fifo,
        socket,
        dir.join("file"),
        dir.join("link"),
        dir.path(),
    ];
    for path in &paths {
        let lite = lfs::symlink_metadata(path).unwrap();
        let real = std::fs::symlink_metadata(path).unwrap();
        same_metadata(path, &lite, &real);
        let (lt, rt) = (lite.file_type(), real.file_type());
        assert_eq!(
            [
                lt.is_block_device(),
                lt.is_char_device(),
                lt.is_fifo(),
                lt.is_socket()
            ],
            [
                rt.is_block_device(),
                rt.is_char_device(),
                rt.is_fifo(),
                rt.is_socket()
            ],
            "{path}"
        );
        assert_eq!(
            format!("{:?}", lite.permissions()),
            format!("{:?}", real.permissions())
        );
        assert_eq!(
            lt == lfs::metadata(path).map_or(lt, |m| m.file_type()),
            rt == std::fs::metadata(path).map_or(rt, |m| m.file_type())
        );
    }
    // Entries report the same types without a system call.
    for entry in lfs::read_dir(dir.path()).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        assert_eq!(
            entry.file_type().unwrap(),
            lfs::symlink_metadata(&path).unwrap().file_type()
        );
    }
    if let Ok(block) = std::fs::symlink_metadata("/dev/loop0") {
        let lite = lfs::symlink_metadata("/dev/loop0").unwrap();
        assert_eq!(
            lite.file_type().is_block_device(),
            block.file_type().is_block_device()
        );
        same_metadata("/dev/loop0", &lite, &block);
    }
}

#[test]
fn symlinks_and_canonicalize_match_std() {
    let dir = TempDir::new("symlinks");
    std::fs::create_dir(dir.join("real")).unwrap();
    std::fs::write(dir.join("real/file"), b"x").unwrap();
    litestd::os::unix::fs::symlink("real", dir.join("link")).unwrap();
    litestd::os::unix::fs::symlink(dir.join("real/file"), dir.join("abs"))
        .unwrap();
    litestd::os::unix::fs::symlink("nowhere", dir.join("dangling")).unwrap();
    litestd::os::unix::fs::symlink("loop", dir.join("loop")).unwrap();
    let long_target = "t".repeat(300);
    litestd::os::unix::fs::symlink(&long_target, dir.join("long")).unwrap();
    // Longer than a first read's buffer; macOS and the BSDs keep targets
    // below 1024 bytes, Linux below 4096.
    let depth = if cfg!(not(any(target_os = "linux", target_os = "android"))) {
        500
    } else {
        1500
    };
    let longest_target = format!("/{}", "x/".repeat(depth));
    litestd::os::unix::fs::symlink(&longest_target, dir.join("longest"))
        .unwrap();
    same(
        "symlink exists",
        litestd::os::unix::fs::symlink("x", dir.join("link")),
        std::os::unix::fs::symlink("x", dir.join("link")),
    );
    for name in [
        "link",
        "abs",
        "dangling",
        "loop",
        "long",
        "longest",
        "real",
        "missing",
        "link/file",
        "link/../real/file",
        "loop/x",
    ] {
        let path = dir.join(name);
        if let Some((a, b)) = same(
            &format!("read_link {name}"),
            lfs::read_link(&path),
            std::fs::read_link(&path),
        ) {
            assert_eq!(a.to_str(), b.to_str(), "{name}");
        }
        if let Some((a, b)) = same(
            &format!("canonicalize {name}"),
            lfs::canonicalize(&path),
            std::fs::canonicalize(&path),
        ) {
            assert_eq!(a.to_str(), b.to_str(), "{name}");
        }
        if let Some((a, b)) = same(
            &format!("metadata {name}"),
            lfs::metadata(&path),
            std::fs::metadata(&path),
        ) {
            same_metadata(name, &a, &b);
        }
        if let Some((a, b)) = same(
            &format!("symlink_metadata {name}"),
            lfs::symlink_metadata(&path),
            std::fs::symlink_metadata(&path),
        ) {
            same_metadata(name, &a, &b);
        }
    }
    assert_eq!(
        lfs::read_link(dir.join("long")).unwrap().to_str(),
        Some(long_target.as_str())
    );
    assert!(
        lfs::symlink_metadata(dir.join("link"))
            .unwrap()
            .is_symlink()
    );
    assert!(lfs::metadata(dir.join("link")).unwrap().is_dir());
    // `hard_link` links a symlink itself.
    lfs::hard_link(dir.join("dangling"), dir.join("hard")).unwrap();
    assert!(
        std::fs::symlink_metadata(dir.join("hard"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
#[cfg_attr(miri, ignore = "Miri does not implement chown")]
fn ownership_matches_std() {
    let dir = TempDir::new("chown");
    let path = dir.join("file");
    std::fs::write(&path, b"x").unwrap();
    std::os::unix::fs::symlink("file", dir.join("link")).unwrap();
    let meta = std::fs::metadata(&path).unwrap();
    let (uid, gid) = (meta.uid(), meta.gid());
    litestd::os::unix::fs::chown(&path, Some(uid), Some(gid)).unwrap();
    litestd::os::unix::fs::chown(&path, None, None).unwrap();
    litestd::os::unix::fs::lchown(dir.join("link"), Some(uid), None).unwrap();
    let file = lfs::File::open(&path).unwrap();
    litestd::os::unix::fs::fchown(&file, None, Some(gid)).unwrap();
    same(
        "chown missing",
        litestd::os::unix::fs::chown(dir.join("missing"), None, None),
        std::os::unix::fs::chown(dir.join("missing"), None, None),
    );
    if !is_root() {
        same(
            "chown to root",
            litestd::os::unix::fs::chown(&path, Some(0), None),
            std::os::unix::fs::chown(&path, Some(0), None),
        );
    }
    let meta = std::fs::metadata(&path).unwrap();
    assert_eq!((meta.uid(), meta.gid()), (uid, gid));
}

#[test]
#[cfg_attr(miri, ignore = "Miri does not implement chroot")]
fn chroot_errors_match_std() {
    let dir = TempDir::new("chroot");
    same(
        "chroot missing",
        litestd::os::unix::fs::chroot(dir.join("missing")),
        std::os::unix::fs::chroot(dir.join("missing")),
    );
}

#[test]
#[cfg_attr(miri, ignore = "Miri does not implement openat and getdents64")]
fn dir_entry_ext_matches_std() {
    use litestd::os::unix::fs::DirEntryExt as _;
    let dir = TempDir::new("dir-entry-ext");
    std::fs::write(dir.join("file"), b"x").unwrap();
    let entry = lfs::read_dir(dir.path()).unwrap().next().unwrap().unwrap();
    assert_eq!(
        entry.ino(),
        std::fs::metadata(dir.join("file")).unwrap().ino()
    );
}

/// Read-only files stay readable through `File::open` and writable by no
/// one but root.
#[test]
fn readonly_files() {
    let dir = TempDir::new("readonly");
    let path = dir.join("file");
    std::fs::write(&path, b"x").unwrap();
    lfs::set_permissions(&path, lfs::Permissions::from_mode(0o444)).unwrap();
    assert!(lfs::metadata(&path).unwrap().permissions().readonly());
    if !is_root() {
        same(
            "write read-only",
            lfs::File::create(&path).map(drop),
            std::fs::File::create(&path).map(drop),
        );
    }
    assert_eq!(lfs::read(&path).unwrap(), b"x");
}
