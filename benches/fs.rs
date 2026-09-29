//! litestd's filesystem, environment and paths against std's. Everything
//! written stays in the benchmark's own temporary directory.

#![allow(clippy::unwrap_used, reason = "a failure ends the benchmark")]

mod common;

use core::hint::black_box;
use std::path::PathBuf;

use common::compare;

/// A unique directory under the system's temporary directory, removed when
/// dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let name = format!("litestd-bench-fs-{}", std::process::id());
        let path = std::env::temp_dir().join(name);
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    /// The path of `name` in the directory, as litestd's paths accept it.
    fn file(&self, name: &str) -> String {
        self.0.join(name).into_os_string().into_string().unwrap()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn main() {
    let dir = TempDir::new();
    metadata(&dir);
    reads(&dir);
    writes(&dir);
    directories(&dir);
    environment();
    paths();
}

fn metadata(dir: &TempDir) {
    let file = dir.file("small");
    std::fs::write(&file, b"small file").unwrap();
    compare(
        "fs metadata",
        || drop(black_box(litestd::fs::metadata(black_box(&*file)))),
        || drop(black_box(std::fs::metadata(black_box(&file)))),
    );
    #[cfg(unix)]
    let link = {
        let link = dir.file("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        link
    };
    #[cfg(not(unix))]
    let link = file;
    compare(
        "fs symlink_metadata",
        || drop(black_box(litestd::fs::symlink_metadata(black_box(&*link)))),
        || drop(black_box(std::fs::symlink_metadata(black_box(&link)))),
    );
}

fn reads(dir: &TempDir) {
    let small = dir.file("small");
    let (mut lite_buf, mut std_buf) = ([0; 64], [0; 64]);
    compare(
        "fs open and read small file",
        || {
            let mut file = litestd::fs::File::open(black_box(&*small)).unwrap();
            let n = litestd::io::Read::read(&mut file, &mut lite_buf).unwrap();
            black_box(n);
        },
        || {
            let mut file = std::fs::File::open(black_box(&small)).unwrap();
            black_box(std::io::Read::read(&mut file, &mut std_buf).unwrap());
        },
    );
    let large = dir.file("large");
    std::fs::write(&large, vec![7; 1 << 20]).unwrap();
    compare(
        "fs read 1 MiB",
        || drop(black_box(litestd::fs::read(black_box(&*large)).unwrap())),
        || drop(black_box(std::fs::read(black_box(&large)).unwrap())),
    );
    let nested = dir.file("sub/../small");
    std::fs::create_dir(dir.file("sub")).unwrap();
    compare(
        "fs canonicalize",
        || drop(black_box(litestd::fs::canonicalize(black_box(&*nested)))),
        || drop(black_box(std::fs::canonicalize(black_box(&nested)))),
    );
}

fn writes(dir: &TempDir) {
    let data = vec![1; 4096];
    let written = dir.file("written");
    compare(
        "fs write 4 KiB",
        || litestd::fs::write(black_box(&*written), &data).unwrap(),
        || std::fs::write(black_box(&written), &data).unwrap(),
    );
    let copy = dir.file("copy");
    compare(
        "fs copy 4 KiB",
        || {
            black_box(litestd::fs::copy(&*written, &*copy).unwrap());
        },
        || {
            black_box(std::fs::copy(&written, &copy).unwrap());
        },
    );
    let renamed = dir.file("renamed");
    compare(
        "fs rename there and back",
        || {
            litestd::fs::rename(&*written, &*renamed).unwrap();
            litestd::fs::rename(&*renamed, &*written).unwrap();
        },
        || {
            std::fs::rename(&written, &renamed).unwrap();
            std::fs::rename(&renamed, &written).unwrap();
        },
    );
}

fn directories(dir: &TempDir) {
    let created = dir.file("created");
    compare(
        "fs create_dir and remove_dir",
        || {
            litestd::fs::create_dir(black_box(&*created)).unwrap();
            litestd::fs::remove_dir(black_box(&*created)).unwrap();
        },
        || {
            std::fs::create_dir(black_box(&created)).unwrap();
            std::fs::remove_dir(black_box(&created)).unwrap();
        },
    );
    let many = dir.file("many");
    std::fs::create_dir(&many).unwrap();
    for i in 0..1000 {
        std::fs::write(format!("{many}/entry-{i}"), b"").unwrap();
    }
    compare(
        "fs read_dir 1000 entries",
        || {
            for entry in litestd::fs::read_dir(black_box(&*many)).unwrap() {
                black_box(entry.unwrap().file_name());
            }
        },
        || {
            for entry in std::fs::read_dir(black_box(&many)).unwrap() {
                black_box(entry.unwrap().file_name());
            }
        },
    );
}

fn environment() {
    const SET: &str = "LITESTD_BENCH_SET";
    const UNSET: &str = "LITESTD_BENCH_SURELY_UNSET";
    // SAFETY: no other thread runs yet.
    unsafe { std::env::set_var(SET, "a value of about the length of a path") };
    compare(
        "env var hit",
        || drop(black_box(litestd::env::var(black_box(SET)))),
        || drop(black_box(std::env::var(black_box(SET)))),
    );
    compare(
        "env var miss",
        || drop(black_box(litestd::env::var(black_box(UNSET)))),
        || drop(black_box(std::env::var(black_box(UNSET)))),
    );
    compare(
        "env vars iterate",
        || litestd::env::vars().for_each(|pair| drop(black_box(pair))),
        || std::env::vars().for_each(|pair| drop(black_box(pair))),
    );
    compare(
        "env args collect",
        || drop(black_box(litestd::env::args().collect::<Vec<_>>())),
        || drop(black_box(std::env::args().collect::<Vec<_>>())),
    );
    compare(
        "env current_dir",
        || drop(black_box(litestd::env::current_dir().unwrap())),
        || drop(black_box(std::env::current_dir().unwrap())),
    );
}

fn paths() {
    use std::path::Path as StdPath;

    use litestd::path::Path as LitePath;

    const LONG: &str =
        "/usr/local/lib/rustlib/x86_64-unknown-linux-gnu/lib/libstd.rlib";
    compare(
        "path components",
        || {
            black_box(LitePath::new(black_box(LONG)).components().count());
        },
        || {
            black_box(StdPath::new(black_box(LONG)).components().count());
        },
    );
    compare(
        "path join",
        || {
            let base = LitePath::new(black_box("/usr/local"));
            drop(black_box(base.join("lib/rustlib")));
        },
        || {
            let base = StdPath::new(black_box("/usr/local"));
            drop(black_box(base.join("lib/rustlib")));
        },
    );
    compare(
        "path extension",
        || {
            black_box(LitePath::new(black_box(LONG)).extension());
        },
        || {
            black_box(StdPath::new(black_box(LONG)).extension());
        },
    );
    compare(
        "path file_name",
        || {
            black_box(LitePath::new(black_box(LONG)).file_name());
        },
        || {
            black_box(StdPath::new(black_box(LONG)).file_name());
        },
    );
}
