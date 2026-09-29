//! Directories: `read_dir`, `DirEntry`, `create_dir_all` and
//! `remove_dir_all`, compared with std, and `remove_dir_all`'s immunity to
//! symlink races.

// wasm32-unknown-unknown has no file system: `tests/unsupported.rs`
// compares its errors with std's.
#![cfg(all(feature = "fs", not(target_os = "unknown")))]
#![allow(clippy::unwrap_used, reason = "helpers, like tests, fail on errors")]

extern crate alloc;

mod fs_util;

use alloc::collections::BTreeMap;
#[cfg(unix)]
use alloc::sync::Arc;
#[cfg(unix)]
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
#[cfg(unix)]
use std::sync::Barrier;
use std::thread;

use fs_util::{TempDir, same, same_metadata};
use litestd::fs as lfs;

/// What a listing reports about one entry.
#[derive(Debug, PartialEq, Eq)]
struct Entry {
    path: String,
    kind: (bool, bool, bool),
    #[cfg(unix)]
    ino: u64,
}

fn lite_listing(dir: &str) -> BTreeMap<String, Entry> {
    let mut entries = BTreeMap::new();
    for entry in lfs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap();
        let kind = entry.file_type().unwrap();
        let meta = entry.metadata().unwrap();
        assert_eq!(
            (kind.is_file(), kind.is_dir(), kind.is_symlink()),
            (meta.is_file(), meta.is_dir(), meta.is_symlink()),
            "{name}"
        );
        let std_meta =
            std::fs::symlink_metadata(entry.path().to_str().unwrap()).unwrap();
        same_metadata(&name, &meta, &std_meta);
        let listed = Entry {
            path: entry.path().into_os_string().into_string().unwrap(),
            kind: (kind.is_file(), kind.is_dir(), kind.is_symlink()),
            #[cfg(unix)]
            ino: litestd::os::unix::fs::DirEntryExt::ino(&entry),
        };
        assert!(entries.insert(name, listed).is_none());
    }
    entries
}

fn std_listing(dir: &str) -> BTreeMap<String, Entry> {
    let mut entries = BTreeMap::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap();
        let kind = entry.file_type().unwrap();
        let listed = Entry {
            path: entry.path().into_os_string().into_string().unwrap(),
            kind: (kind.is_file(), kind.is_dir(), kind.is_symlink()),
            #[cfg(unix)]
            ino: std::os::unix::fs::DirEntryExt::ino(&entry),
        };
        entries.insert(name, listed);
    }
    entries
}

#[test]
#[cfg_attr(miri, ignore = "Miri does not implement openat and getdents64")]
fn read_dir_matches_std() {
    let dir = TempDir::new("read-dir");
    std::fs::write(dir.join("file"), b"x").unwrap();
    std::fs::write(dir.join("empty"), b"").unwrap();
    std::fs::create_dir(dir.join("sub")).unwrap();
    // Longer than the inline name storage, and as long as names get.
    std::fs::write(dir.join(&"n".repeat(60)), b"long").unwrap();
    std::fs::write(dir.join(&"m".repeat(255)), b"longest").unwrap();
    std::fs::write(dir.join(&"k".repeat(21)), b"inline").unwrap();
    std::fs::write(dir.join(&"j".repeat(22)), b"heap").unwrap();
    std::fs::write(dir.join("caf\u{e9} \u{1F496}"), b"utf-8").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("file", dir.join("link")).unwrap();
        std::os::unix::fs::symlink("nowhere", dir.join("dangling")).unwrap();
        std::os::unix::fs::symlink("sub", dir.join("dirlink")).unwrap();
    }
    let lite = lite_listing(&dir.path());
    assert_eq!(lite, std_listing(&dir.path()));
    assert!(lite.len() >= 8);
    let empty = dir.join("sub");
    assert_eq!(lfs::read_dir(&empty).unwrap().count(), 0);
}

/// Enough entries that the kernel returns them over many calls.
#[test]
#[cfg_attr(miri, ignore = "Miri does not implement openat and getdents64")]
fn read_dir_many_entries() {
    let dir = TempDir::new("many");
    for i in 0..3000 {
        std::fs::write(
            dir.join(&format!("entry-with-a-longish-name-{i:05}")),
            b"",
        )
        .unwrap();
    }
    let lite = lite_listing(&dir.path());
    assert_eq!(lite.len(), 3000);
    assert_eq!(lite, std_listing(&dir.path()));
}

#[test]
#[cfg_attr(miri, ignore = "Miri does not implement openat and getdents64")]
fn read_dir_errors_and_debug_match_std() {
    let dir = TempDir::new("read-dir-errors");
    std::fs::write(dir.join("file"), b"x").unwrap();
    for name in ["missing", "file", "file/x", ""] {
        let path = if name.is_empty() {
            String::new()
        } else {
            dir.join(name)
        };
        same(
            name,
            lfs::read_dir(&path).map(drop),
            std::fs::read_dir(&path).map(drop),
        );
    }
    let lite = lfs::read_dir(dir.path()).unwrap();
    let real = std::fs::read_dir(dir.path()).unwrap();
    assert_eq!(format!("{lite:?}"), format!("{real:?}"));
    let lite = lfs::read_dir(dir.path()).unwrap().next().unwrap().unwrap();
    let real = std::fs::read_dir(dir.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(format!("{lite:?}"), format!("{real:?}"));
    // Relative paths stay relative.
    let lite = lfs::read_dir(".")
        .unwrap()
        .next()
        .map(|e| e.unwrap().path());
    if let Some(path) = lite {
        assert!(path.starts_with("."), "{path:?}");
    }
}

/// Entries outlive their stream and keep the directory open; their metadata
/// is looked up relative to it, even after it moved.
#[test]
#[cfg_attr(miri, ignore = "Miri does not implement openat and getdents64")]
#[cfg_attr(
    target_os = "wasi",
    ignore = "WASI runtimes may track directories by path"
)]
fn entries_outlive_the_stream() {
    let dir = TempDir::new("outlive");
    std::fs::create_dir(dir.join("a")).unwrap();
    std::fs::write(dir.join("a/file"), b"12345").unwrap();
    let mut stream = lfs::read_dir(dir.join("a")).unwrap();
    let entry = stream.next().unwrap().unwrap();
    assert!(stream.next().is_none());
    assert!(stream.next().is_none());
    drop(stream);
    std::fs::rename(dir.join("a"), dir.join("b")).unwrap();
    assert_eq!(entry.metadata().unwrap().len(), 5);
    assert_eq!(entry.file_name(), "file");
    // The path is built from the name the directory was opened with.
    let expected = std::path::Path::new(&dir.join("a")).join("file");
    assert_eq!(entry.path().to_str(), expected.to_str());
}

#[test]
fn create_dir_matches_std() {
    let dir = TempDir::new("create-dir");
    std::fs::write(dir.join("file"), b"x").unwrap();
    lfs::create_dir(dir.join("new")).unwrap();
    assert!(std::fs::metadata(dir.join("new")).unwrap().is_dir());
    for name in ["new", "file", "missing/new", "file/new"] {
        same(
            name,
            lfs::create_dir(dir.join(name)),
            std::fs::create_dir(dir.join(name)),
        );
    }
    for name in [
        "new",
        "file",
        "file/new",
        "a/b/c/d",
        "a/b/c/d/",
        "a/./b/../e",
        "file/x/y",
    ] {
        let lite = lfs::create_dir_all(dir.join(name));
        let real = std::fs::create_dir_all(dir.join(name));
        same(name, lite, real);
    }
    assert!(std::fs::metadata(dir.join("a/b/c/d")).unwrap().is_dir());
    assert!(std::fs::metadata(dir.join("a/e")).unwrap().is_dir());
    lfs::create_dir_all("").unwrap();
    lfs::create_dir_all("/").unwrap();
    let mut builder = lfs::DirBuilder::new();
    same(
        "not recursive",
        builder.create(dir.join("x/y")),
        std::fs::DirBuilder::new().create(dir.join("x/y")),
    );
    builder.recursive(true).create(dir.join("x/y")).unwrap();
    assert!(std::fs::metadata(dir.join("x/y")).unwrap().is_dir());
    assert_eq!(
        format!("{builder:?}"),
        format!("{:?}", std::fs::DirBuilder::new().recursive(true))
    );
}

/// Many threads creating overlapping trees all succeed.
#[test]
#[cfg_attr(miri, ignore = "slow")]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn create_dir_all_tolerates_races() {
    let dir = TempDir::new("create-race");
    let base = dir.path();
    for round in 0..20 {
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let path = format!("{base}/r{round}/a/b/c/{}", i % 2);
                thread::spawn(move || lfs::create_dir_all(path))
            })
            .collect();
        for t in threads {
            t.join().unwrap().unwrap();
        }
        assert!(
            std::fs::metadata(format!("{base}/r{round}/a/b/c/1"))
                .unwrap()
                .is_dir()
        );
    }
}

/// Builds a tree of files, directories and, on Unix, symlinks under `root`.
fn build_tree(root: &str, depth: usize) {
    std::fs::create_dir_all(root).unwrap();
    for i in 0..3 {
        std::fs::write(format!("{root}/file{i}"), b"data").unwrap();
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink("file0", format!("{root}/link")).unwrap();
    if depth > 0 {
        build_tree(&format!("{root}/dir"), depth - 1);
        build_tree(&format!("{root}/other"), depth - 1);
    }
}

#[test]
#[cfg_attr(miri, ignore = "Miri does not implement openat and getdents64")]
fn remove_dir_all_removes_trees() {
    let dir = TempDir::new("remove-all");
    let root = dir.join("tree");
    build_tree(&root, 4);
    std::fs::create_dir_all(dir.join("tree/empty/deeper")).unwrap();
    lfs::remove_dir_all(&root).unwrap();
    assert!(!std::path::Path::new(&root).exists());
    for name in ["tree", "tree/x"] {
        let (lite, real) = (dir.join(name), dir.join(name));
        same(
            name,
            lfs::remove_dir_all(&lite),
            std::fs::remove_dir_all(&real),
        );
    }
    std::fs::write(dir.join("file"), b"x").unwrap();
    same(
        "a file",
        lfs::remove_dir_all(dir.join("file")),
        std::fs::remove_dir_all(dir.join("file")),
    );
    assert!(std::path::Path::new(&dir.join("file")).exists());
    std::fs::create_dir(dir.join("empty")).unwrap();
    lfs::remove_dir_all(dir.join("empty")).unwrap();
    assert!(!std::path::Path::new(&dir.join("empty")).exists());
}

/// A symlink passed to `remove_dir_all` is removed, not followed.
#[cfg(unix)]
#[test]
#[cfg_attr(miri, ignore = "Miri does not implement openat and getdents64")]
fn remove_dir_all_removes_a_symlink_root() {
    let dir = TempDir::new("remove-link-root");
    build_tree(&dir.join("target"), 1);
    std::os::unix::fs::symlink(dir.join("target"), dir.join("link")).unwrap();
    lfs::remove_dir_all(dir.join("link")).unwrap();
    assert!(std::fs::symlink_metadata(dir.join("link")).is_err());
    assert_eq!(lite_file_count(&dir.join("target")), 9);
}

/// Counts the regular files under `root`, with std.
#[cfg(unix)]
fn lite_file_count(root: &str) -> usize {
    let mut count = 0;
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            count += lite_file_count(entry.path().to_str().unwrap());
        } else if kind.is_file() {
            count += 1;
        }
    }
    count
}

/// Symlinks inside the tree that point outside of it, to directories and to
/// files, are removed themselves; their targets stay untouched.
#[cfg(unix)]
#[test]
#[cfg_attr(miri, ignore = "Miri does not implement openat and getdents64")]
fn remove_dir_all_never_follows_symlinks() {
    let dir = TempDir::new("remove-links");
    let outside = dir.join("outside");
    build_tree(&outside, 2);
    let before = lite_file_count(&outside);
    let tree = dir.join("tree");
    build_tree(&tree, 2);
    let links = [
        ("tree/to-dir", outside.clone()),
        ("tree/dir/to-dir", format!("{outside}/dir")),
        ("tree/to-file", format!("{outside}/file0")),
        ("tree/dir/other/to-parent", dir.path()),
        ("tree/relative", "../outside".to_owned()),
        ("tree/to-root", "/".to_owned()),
    ];
    for (link, target) in &links {
        std::os::unix::fs::symlink(target, dir.join(link)).unwrap();
    }
    lfs::remove_dir_all(&tree).unwrap();
    assert!(!std::path::Path::new(&tree).exists());
    assert_eq!(lite_file_count(&outside), before);
    assert_eq!(std::fs::read(format!("{outside}/file0")).unwrap(), b"data");
}

/// Another thread keeps replacing directories of the tree with symlinks to a
/// directory outside of it while `remove_dir_all` runs: the outside
/// directory must never lose a file (CVE-2022-21658). The same attack makes
/// a remover that checks with `metadata` before recursing delete files
/// outside the tree in nearly every round.
#[cfg(unix)]
#[test]
#[cfg_attr(miri, ignore = "Miri does not implement openat and getdents64")]
fn remove_dir_all_resists_symlink_races() {
    let dir = TempDir::new("remove-race");
    let outside = dir.join("outside");
    std::fs::create_dir(&outside).unwrap();
    for i in 0..50 {
        std::fs::write(format!("{outside}/keep{i}"), b"keep").unwrap();
    }
    let swaps = Arc::new(AtomicUsize::new(0));
    for round in 0..40 {
        let tree = dir.join(&format!("tree{round}"));
        for i in 0..20 {
            let sub = format!("{tree}/sub{i}");
            std::fs::create_dir_all(&sub).unwrap();
            for j in 0..10 {
                std::fs::write(format!("{sub}/f{j}"), b"x").unwrap();
            }
        }
        let stop = Arc::new(AtomicBool::new(false));
        let start = Arc::new(Barrier::new(2));
        let attacker = {
            let (tree, outside) = (tree.clone(), outside.clone());
            let (stop, start, swaps) =
                (Arc::clone(&stop), Arc::clone(&start), Arc::clone(&swaps));
            thread::spawn(move || {
                start.wait();
                let mut i = 0;
                while !stop.load(Ordering::Relaxed) {
                    let sub = format!("{tree}/sub{}", i % 20);
                    // Swap the directory for a symlink to `outside`; losing
                    // the race to the removal is fine.
                    if std::fs::rename(&sub, format!("{sub}.moved")).is_ok()
                        && std::os::unix::fs::symlink(&outside, &sub).is_ok()
                    {
                        swaps.fetch_add(1, Ordering::Relaxed);
                    }
                    i += 1;
                }
            })
        };
        start.wait();
        // The removal may fail when the attacker recreates entries behind it;
        // it must never touch `outside`.
        let _ = lfs::remove_dir_all(&tree);
        stop.store(true, Ordering::Relaxed);
        attacker.join().unwrap();
        for i in 0..50 {
            assert_eq!(
                std::fs::read(format!("{outside}/keep{i}")).unwrap(),
                b"keep",
                "round {round}: a file outside the tree was removed"
            );
        }
        let _ = std::fs::remove_dir_all(&tree);
    }
    assert!(swaps.load(Ordering::Relaxed) > 0, "the attack never ran");
}

/// Another remover deletes a subdirectory while `remove_dir_all` empties it.
/// Listing a directory that was removed meanwhile fails with `ENOENT`, which
/// std takes as the subdirectory being gone: the removal of the whole tree
/// must still succeed, as it does with std.
#[cfg(unix)]
#[test]
#[cfg_attr(miri, ignore = "Miri does not implement openat and getdents64")]
fn remove_dir_all_tolerates_subdirectories_removed_concurrently() {
    let dir = TempDir::new("remove-concurrent");
    for round in 0..30 {
        let root = dir.join(&format!("root{round}"));
        let sub = format!("{root}/sub");
        std::fs::create_dir_all(&sub).unwrap();
        // Enough entries for several listings, so that the other remover
        // can finish while this one is between two of them.
        for i in 0..1500 {
            std::fs::write(format!("{sub}/entry-with-a-long-name-{i:04}"), b"")
                .unwrap();
        }
        let start = Arc::new(Barrier::new(2));
        let other = {
            let start = Arc::clone(&start);
            thread::spawn(move || {
                start.wait();
                // Fails if the tested removal gets to `sub` first.
                let _ = std::fs::remove_dir_all(&sub);
            })
        };
        start.wait();
        let result = lfs::remove_dir_all(&root);
        other.join().unwrap();
        assert!(result.is_ok(), "round {round}: {result:?}");
        assert!(!std::path::Path::new(&root).exists());
    }
}

/// The walk keeps its state on the heap: a deep tree does not overflow even
/// a small thread stack.
#[test]
#[cfg_attr(miri, ignore = "Miri does not implement openat and getdents64")]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn remove_dir_all_handles_deep_trees_on_a_small_stack() {
    let dir = TempDir::new("deep");
    let root = dir.join("root");
    let depth = 400;
    let deep = format!("{root}{}", "/d".repeat(depth));
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(format!("{deep}/leaf"), b"leaf").unwrap();
    let remover = thread::Builder::new()
        .stack_size(64 * 1024)
        .spawn(move || lfs::remove_dir_all(root))
        .unwrap();
    remover.join().unwrap().unwrap();
    assert!(!std::path::Path::new(&dir.join("root")).exists());
}

#[test]
#[cfg_attr(miri, ignore = "Miri does not implement openat and getdents64")]
fn remove_dir_all_with_many_entries() {
    let dir = TempDir::new("remove-many");
    let root = dir.join("root");
    for i in 0..30 {
        let sub = format!("{root}/sub{i:02}");
        std::fs::create_dir_all(&sub).unwrap();
        for j in 0..100 {
            std::fs::write(
                format!("{sub}/a-file-with-a-long-name-{j:03}"),
                b"",
            )
            .unwrap();
        }
    }
    lfs::remove_dir_all(&root).unwrap();
    assert!(!std::path::Path::new(&root).exists());
}
