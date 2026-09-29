//! The file system methods of `litestd::path::Path` and `path::absolute`,
//! against std's.

// wasm32-unknown-unknown has no file system: `tests/unsupported.rs`
// compares its errors with std's.
#![cfg(all(feature = "fs", not(target_os = "unknown")))]

mod fs_util;

use fs_util::{TempDir, same, same_metadata};
use litestd::path::{self as lpath, Path};

#[test]
fn path_methods_match_std() {
    let dir = TempDir::new("path");
    std::fs::write(dir.join("file"), b"x").unwrap();
    std::fs::create_dir(dir.join("dir")).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("file", dir.join("link")).unwrap();
        std::os::unix::fs::symlink("nowhere", dir.join("dangling")).unwrap();
    }
    for name in ["file", "dir", "link", "dangling", "missing", "file/x", ""] {
        let path = if name.is_empty() {
            String::new()
        } else {
            dir.join(name)
        };
        let (lite, real) = (Path::new(&path), std::path::Path::new(&path));
        assert_eq!(lite.exists(), real.exists(), "{name}: exists");
        assert_eq!(lite.is_file(), real.is_file(), "{name}: is_file");
        assert_eq!(lite.is_dir(), real.is_dir(), "{name}: is_dir");
        assert_eq!(lite.is_symlink(), real.is_symlink(), "{name}: is_symlink");
        if let Some((a, b)) = same(name, lite.try_exists(), real.try_exists()) {
            assert_eq!(a, b, "{name}: try_exists");
        }
        if let Some((a, b)) = same(name, lite.metadata(), real.metadata()) {
            same_metadata(name, &a, &b);
        }
        if let Some((a, b)) =
            same(name, lite.symlink_metadata(), real.symlink_metadata())
        {
            same_metadata(name, &a, &b);
        }
        if let Some((a, b)) =
            same(name, lite.canonicalize(), real.canonicalize())
        {
            assert_eq!(a.to_str(), b.to_str(), "{name}: canonicalize");
        }
        if let Some((a, b)) = same(name, lite.read_link(), real.read_link()) {
            assert_eq!(a.to_str(), b.to_str(), "{name}: read_link");
        }
    }
}

#[test]
#[cfg_attr(miri, ignore = "Miri does not implement openat and getdents64")]
fn read_dir_method_matches_std() {
    let dir = TempDir::new("path-read-dir");
    std::fs::write(dir.join("a"), b"").unwrap();
    std::fs::write(dir.join("b"), b"").unwrap();
    let mut lite: Vec<_> = Path::new(&dir.path())
        .read_dir()
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    lite.sort();
    assert_eq!(lite, ["a", "b"]);
    same(
        "read_dir missing",
        Path::new(&dir.join("missing")).read_dir().map(drop),
        std::path::Path::new(&dir.join("missing"))
            .read_dir()
            .map(drop),
    );
}

#[test]
fn absolute_matches_std() {
    #[cfg(not(windows))]
    let inputs = [
        "/",
        "//",
        "///",
        "/a",
        "//a",
        "///a",
        "/a/b/",
        "/a/./b/../c",
        "/a//b",
        "a",
        "a/b",
        "./a",
        ".",
        "./",
        "..",
        "../a",
        "a/../b",
        "a/./b/",
        "a//",
        ".//a",
    ];
    #[cfg(windows)]
    let inputs = [
        "C:\\",
        "C:\\a\\..\\b",
        "a",
        ".\\a",
        "..\\a",
        "\\\\?\\C:\\a",
        "C:a",
    ];
    for input in inputs {
        let lite = lpath::absolute(input);
        let real = std::path::absolute(input);
        if let Some((a, b)) = same(input, lite, real) {
            assert_eq!(a.to_str(), b.to_str(), "{input}");
        }
    }
    same("empty", lpath::absolute(""), std::path::absolute(""));
}
