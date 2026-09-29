//! `litestd::fs::remove_dir_all` on Windows: whole trees go, and links in
//! them are removed without touching what they point to, even when they
//! replace directories while the removal runs.

#![cfg(all(windows, feature = "fs"))]

mod fs_util;
mod fs_windows_util;

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::{io, path::Path, thread};

use fs_util::{TempDir, same};
use fs_windows_util::{junction, symlink};
use litestd::fs as lfs;

/// A directory outside the trees under test, with files that must survive.
fn outside(dir: &TempDir) -> io::Result<String> {
    let outside = dir.join("outside");
    std::fs::create_dir(&outside)?;
    std::fs::write(format!(r"{outside}\keep.txt"), b"keep")?;
    std::fs::create_dir(format!(r"{outside}\sub"))?;
    let readonly = format!(r"{outside}\sub\keep.txt");
    std::fs::write(&readonly, b"keep")?;
    let mut perm = std::fs::metadata(&readonly)?.permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&readonly, perm)?;
    Ok(outside)
}

/// Asserts that `outside` holds exactly what `outside()` put there.
#[track_caller]
fn assert_intact(outside: &str) {
    for file in [r"keep.txt", r"sub\keep.txt"] {
        let contents = std::fs::read(format!(r"{outside}\{file}"));
        assert_eq!(contents.ok().as_deref(), Some(&b"keep"[..]), "{file}");
    }
    let entries = std::fs::read_dir(outside).map(Iterator::count);
    assert_eq!(entries.ok(), Some(2));
}

/// Builds a tree with files, a read-only file, nested, deep and empty
/// directories.
fn build_tree(root: &str) -> io::Result<()> {
    std::fs::create_dir(root)?;
    for i in 0..40 {
        std::fs::write(format!(r"{root}\file{i}"), [i; 100])?;
    }
    for d in 0..8 {
        let sub = format!(r"{root}\dir{d}");
        std::fs::create_dir(&sub)?;
        for i in 0..15 {
            std::fs::write(format!(r"{sub}\f{i}.txt"), b"x")?;
        }
        std::fs::create_dir(format!(r"{sub}\empty"))?;
    }
    let readonly = format!(r"{root}\dir3\readonly");
    std::fs::write(&readonly, b"ro")?;
    let mut perm = std::fs::metadata(&readonly)?.permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&readonly, perm)?;
    let mut deep = format!(r"{root}\deep");
    for _ in 0..150 {
        deep.push_str(r"\d");
    }
    std::fs::create_dir_all(&deep)?;
    std::fs::write(format!(r"{deep}\bottom"), b"bottom")
}

#[test]
fn removes_whole_trees_like_std() {
    let dir = TempDir::new("rmtree");
    let (lite, real) = (dir.join("lite"), dir.join("std"));
    build_tree(&lite).unwrap();
    build_tree(&real).unwrap();
    same(
        "remove tree",
        lfs::remove_dir_all(&lite),
        std::fs::remove_dir_all(&real),
    );
    assert!(!Path::new(&lite).exists());
    // Removing it again fails the same way.
    same(
        "remove missing",
        lfs::remove_dir_all(&lite),
        std::fs::remove_dir_all(&real),
    );
}

#[test]
fn removes_large_directories() {
    let dir = TempDir::new("rmwide");
    let root = dir.join("wide");
    std::fs::create_dir(&root).unwrap();
    // More entries than one listing call returns, with long names.
    for i in 0..600 {
        std::fs::write(format!(r"{root}\{}{i}", "n".repeat(60)), b"").unwrap();
        if i % 50 == 0 {
            std::fs::create_dir(format!(r"{root}\sub{i}")).unwrap();
            std::fs::write(format!(r"{root}\sub{i}\x"), b"").unwrap();
        }
    }
    lfs::remove_dir_all(&root).unwrap();
    assert!(!Path::new(&root).exists());
}

#[test]
fn errors_match_std() {
    let dir = TempDir::new("rmerr");
    for side in ["lite", "std"] {
        std::fs::write(dir.join(side), b"file").unwrap();
    }
    same(
        "a file",
        lfs::remove_dir_all(dir.join("lite")),
        std::fs::remove_dir_all(dir.join("std")),
    );
    assert!(Path::new(&dir.join("lite")).exists());
    same(
        "missing",
        lfs::remove_dir_all(dir.join("missing")),
        std::fs::remove_dir_all(dir.join("missing")),
    );
    same(
        "missing parent",
        lfs::remove_dir_all(dir.join(r"missing\child")),
        std::fs::remove_dir_all(dir.join(r"missing\child")),
    );
    same(
        "NUL",
        lfs::remove_dir_all(dir.join("a\0b")),
        std::fs::remove_dir_all(dir.join("a\0b")),
    );
}

#[test]
fn open_files_inside_behave_like_std() {
    use std::os::windows::fs::OpenOptionsExt as _;
    /// `FILE_SHARE_READ` alone: other handles may not delete the file.
    const SHARE_READ: u32 = 1;

    let dir = TempDir::new("rmopen");
    for side in ["lite", "std"] {
        std::fs::create_dir(dir.join(side)).unwrap();
        std::fs::write(dir.join(&format!(r"{side}\shared")), b"").unwrap();
        std::fs::write(dir.join(&format!(r"{side}\exclusive")), b"").unwrap();
    }
    // Open with every sharing mode: removable with POSIX semantics.
    let lite_shared = std::fs::File::open(dir.join(r"lite\shared")).unwrap();
    let std_shared = std::fs::File::open(dir.join(r"std\shared")).unwrap();
    // Open without sharing deletion: every attempt fails.
    let exclusive = |path: String| {
        std::fs::File::options()
            .read(true)
            .share_mode(SHARE_READ)
            .open(path)
    };
    let lite_exclusive = exclusive(dir.join(r"lite\exclusive")).unwrap();
    let std_exclusive = exclusive(dir.join(r"std\exclusive")).unwrap();
    same(
        "locked file",
        lfs::remove_dir_all(dir.join("lite")),
        std::fs::remove_dir_all(dir.join("std")),
    );
    drop((lite_exclusive, std_exclusive));
    same(
        "after closing",
        lfs::remove_dir_all(dir.join("lite")),
        std::fs::remove_dir_all(dir.join("std")),
    );
    drop((lite_shared, std_shared));
    assert!(!Path::new(&dir.join("lite")).exists());
}

#[test]
fn links_inside_are_removed_not_followed() {
    let dir = TempDir::new("rmlinks");
    let outside = outside(&dir).unwrap();
    let root = dir.join("tree");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(format!(r"{root}\sub")).unwrap();
    junction(&outside, &format!(r"{root}\junction")).unwrap();
    junction(&outside, &format!(r"{root}\sub\junction")).unwrap();
    junction(
        &format!(r"{outside}\sub"),
        &format!(r"{root}\sub\junction2"),
    )
    .unwrap();
    let file = format!(r"{outside}\keep.txt");
    let symlinks = symlink(&outside, &format!(r"{root}\dir_link"), true)
        .unwrap()
        && symlink(&file, &format!(r"{root}\file_link"), false).unwrap()
        && symlink(r"..\..\outside", &format!(r"{root}\sub\relative"), true)
            .unwrap()
        && symlink(r"C:\does\not\exist", &format!(r"{root}\dangling"), true)
            .unwrap();
    if !symlinks {
        eprintln!("symbolic links need a privilege; testing junctions only");
    }
    lfs::remove_dir_all(&root).unwrap();
    assert!(!Path::new(&root).exists());
    assert_intact(&outside);
}

#[test]
fn a_link_as_root_removes_only_the_link() {
    let dir = TempDir::new("rmroot");
    let outside = outside(&dir).unwrap();
    let link = dir.join("junction");
    junction(&outside, &link).unwrap();
    lfs::remove_dir_all(&link).unwrap();
    assert!(std::fs::symlink_metadata(&link).is_err());
    assert_intact(&outside);
    let link = dir.join("dir_link");
    if symlink(&outside, &link, true).unwrap() {
        lfs::remove_dir_all(&link).unwrap();
        assert!(std::fs::symlink_metadata(&link).is_err());
        assert_intact(&outside);
    }
    // A symbolic link to a file is not a directory, for std either.
    let (link, std_link) = (dir.join("file_link"), dir.join("std_file_link"));
    let file = format!(r"{outside}\keep.txt");
    if symlink(&file, &link, false).unwrap() {
        symlink(&file, &std_link, false).unwrap();
        same(
            "file link as root",
            lfs::remove_dir_all(&link),
            std::fs::remove_dir_all(&std_link),
        );
        assert_intact(&outside);
    }
}

/// Swaps directories of the tree for junctions to `outside` while the
/// removal runs. Whatever the interleaving, the removal must only delete
/// inside the tree: it may fail, but `outside` stays intact.
#[test]
fn junctions_swapped_in_during_removal_are_not_followed() {
    let dir = TempDir::new("rmrace");
    let outside = outside(&dir).unwrap();
    let swaps = AtomicUsize::new(0);
    for round in 0..40 {
        let root = dir.join(&format!("tree{round}"));
        std::fs::create_dir(&root).unwrap();
        for d in 0..6 {
            let sub = format!(r"{root}\d{d}");
            std::fs::create_dir(&sub).unwrap();
            for f in 0..10 {
                std::fs::write(format!(r"{sub}\f{f}"), b"x").unwrap();
            }
        }
        let stop = AtomicBool::new(false);
        let result = thread::scope(|scope| {
            scope.spawn(|| {
                // Each pass tries every subdirectory once; the removal ends
                // the passes by setting `stop`.
                while !stop.load(Ordering::Relaxed) {
                    for d in 0..6 {
                        let sub = format!(r"{root}\d{d}");
                        let n = swaps.load(Ordering::Relaxed);
                        let moved = format!(r"{root}\moved{d}-{n}");
                        if std::fs::rename(&sub, &moved).is_ok()
                            && junction(&outside, &sub).is_ok()
                        {
                            swaps.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    thread::yield_now();
                }
            });
            let result = lfs::remove_dir_all(&root);
            stop.store(true, Ordering::Relaxed);
            result
        });
        assert_intact(&outside);
        // Whatever the race left behind goes now, still without following.
        if result.is_err() || Path::new(&root).exists() {
            lfs::remove_dir_all(&root).unwrap();
        }
        assert!(std::fs::symlink_metadata(&root).is_err());
        assert_intact(&outside);
    }
    // Some junctions went in; how many raced with the removal depends on
    // the scheduling.
    let swaps = swaps.into_inner();
    eprintln!("{swaps} directories swapped for junctions");
    assert!(swaps > 0);
}

#[test]
fn verbatim_and_long_roots() {
    let dir = TempDir::new("rmlong");
    let root = dir.join("long");
    let mut deep = root.clone();
    for _ in 0..8 {
        deep.push('\\');
        deep.push_str(&"x".repeat(50));
    }
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(format!(r"{deep}\file"), b"x").unwrap();
    lfs::remove_dir_all(format!(r"\\?\{root}")).unwrap();
    assert!(!Path::new(&root).exists());
}

#[test]
fn directories_open_inside_behave_like_std() {
    use std::os::windows::fs::OpenOptionsExt as _;
    /// `FILE_SHARE_READ | FILE_SHARE_WRITE`, as a working directory is
    /// held: other handles may not delete the directory.
    const SHARE_READ_WRITE: u32 = 3;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

    let dir = TempDir::new("rmdiropen");
    for side in ["lite", "std"] {
        std::fs::create_dir_all(dir.join(&format!(r"{side}\held\inner")))
            .unwrap();
        std::fs::write(dir.join(&format!(r"{side}\held\inner\file")), b"")
            .unwrap();
    }
    let hold = |path: String| {
        std::fs::File::options()
            .read(true)
            .share_mode(SHARE_READ_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
    };
    let lite_held = hold(dir.join(r"lite\held")).unwrap();
    let std_held = hold(dir.join(r"std\held")).unwrap();
    same(
        "held directory",
        lfs::remove_dir_all(dir.join("lite")),
        std::fs::remove_dir_all(dir.join("std")),
    );
    drop((lite_held, std_held));
    same(
        "after closing",
        lfs::remove_dir_all(dir.join("lite")),
        std::fs::remove_dir_all(dir.join("std")),
    );
    assert!(!Path::new(&dir.join("lite")).exists());
}

/// Set in the child process of `empty_path_behaves_like_std`.
const EMPTY_PATH_CHILD: &str = "LITESTD_EMPTY_PATH_CHILD";

/// Compares `remove_dir_all("")` with std's in a child process whose current
/// directory is a scratch directory. An empty path names nothing, but a
/// regression that resolved it against the current directory could then
/// only remove that scratch directory, never the one the tests run in.
#[test]
fn empty_path_behaves_like_std() {
    if std::env::var_os(EMPTY_PATH_CHILD).is_some() {
        same(
            "empty path",
            lfs::remove_dir_all(""),
            std::fs::remove_dir_all(""),
        );
        return;
    }
    let dir = TempDir::new("rmempty");
    let cwd = dir.join("cwd");
    std::fs::create_dir(&cwd).unwrap();
    let sentinel = format!(r"{cwd}\keep.txt");
    std::fs::write(&sentinel, b"keep").unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "empty_path_behaves_like_std", "--test-threads=1"])
        .env(EMPTY_PATH_CHILD, "1")
        .current_dir(&cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "the child's comparison failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let kept = std::fs::read(&sentinel).ok();
    assert_eq!(kept.as_deref(), Some(&b"keep"[..]), "the scratch directory");
}
