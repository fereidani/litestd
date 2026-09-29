//! `read_to_end` and `read_to_string` into uninitialized memory, through
//! `fs::File` and `fs::read`, against std. Under Miri the tests need
//! `-Zmiri-disable-isolation` for the files.

// wasm32-unknown-unknown has no file system: `tests/unsupported.rs`
// compares its errors with std's.
#![cfg(all(feature = "io", feature = "fs", not(target_os = "unknown")))]

mod io_support;

use core::sync::atomic::{AtomicUsize, Ordering};
use std::path::PathBuf;

use io_support::{Norm, noise, text};

/// A file of its own for one test, which std removes when dropped.
struct TempFile(PathBuf);

#[allow(clippy::unwrap_used, reason = "a failure fails the test")]
impl TempFile {
    fn new(contents: &[u8]) -> Self {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        // WASI has no process ids or temporary directory in std; its runner
        // gives each program a `/tmp` of its own.
        let (base, id) = if cfg!(target_os = "wasi") {
            (PathBuf::from("/tmp"), 0)
        } else {
            (std::env::temp_dir(), std::process::id())
        };
        let path = base.join(format!("litestd-io-{id}-{n}"));
        std::fs::write(&path, contents).unwrap();
        Self(path)
    }

    fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        // A leftover file in the temporary directory is harmless.
        let _ = std::fs::remove_file(&self.0);
    }
}

const fn lengths() -> &'static [usize] {
    if cfg!(miri) {
        &[0, 1, 31, 32, 33, 300]
    } else {
        &[0, 1, 31, 32, 33, 4095, 8192, 70_000, 1 << 20]
    }
}

#[test]
fn file_read_to_end_matches_std() {
    for &len in lengths() {
        let file = TempFile::new(&noise(len, len as u64));
        for position in [0, 1, len / 2, len, len + 5] {
            for prefix in [&b""[..], b"prefix"] {
                differential!(io, fs => {
                    use io::{Read, Seek};
                    let mut f = fs::File::open(file.path()).unwrap();
                    f.seek(io::SeekFrom::Start(position as u64)).unwrap();
                    let mut buf = prefix.to_vec();
                    let result = f.read_to_end(&mut buf).norm();
                    // Reading on at EOF finds nothing more.
                    let again = f.read_to_end(&mut buf).norm();
                    (result, again, buf)
                });
            }
        }
    }
}

#[test]
fn file_read_to_string_matches_std() {
    let mut contents =
        vec![text(0), text(1), text(33), text(9000), noise(100, 1)];
    let mut truncated = text(50);
    truncated.extend_from_slice(&"\u{4e2d}".as_bytes()[..2]);
    contents.push(truncated);
    for data in &contents {
        let file = TempFile::new(data);
        for position in [0, 3] {
            for prefix in ["", "d\u{e9}j\u{e0} "] {
                differential!(io, fs => {
                    use io::{Read, Seek};
                    let mut f = fs::File::open(file.path()).unwrap();
                    f.seek(io::SeekFrom::Start(position)).unwrap();
                    let mut buf = String::from(prefix);
                    let result = f.read_to_string(&mut buf).norm();
                    (result, buf)
                });
            }
        }
    }
}

#[test]
fn file_read_to_end_fills_an_exact_fit() {
    for &len in lengths() {
        let data = noise(len, 3);
        let file = TempFile::new(&data);
        let mut buf = Vec::with_capacity(len);
        let n = litestd::io::Read::read_to_end(
            &mut litestd::fs::File::open(file.path()).unwrap(),
            &mut buf,
        )
        .unwrap();
        assert_eq!((n, buf.capacity()), (len, len));
        assert_eq!(buf, data);
    }
}

#[test]
fn fs_read_matches_std() {
    for &len in lengths() {
        let data = text(len);
        let file = TempFile::new(&data);
        assert_eq!(litestd::fs::read(file.path()).unwrap(), data);
        // `text` may end inside a character, which both reject.
        assert_eq!(
            litestd::fs::read_to_string(file.path()).norm(),
            std::fs::read_to_string(file.path()).norm()
        );
    }
    let file = TempFile::new(b"\xff invalid");
    assert_eq!(
        litestd::fs::read_to_string(file.path()).norm(),
        std::fs::read_to_string(file.path()).norm()
    );
}

#[test]
fn buffered_file_matches_std() {
    let data = text(if cfg!(miri) { 500 } else { 100_000 });
    let file = TempFile::new(&data);
    differential!(io, fs => {
        use io::{BufRead, Read};
        let f = fs::File::open(file.path()).unwrap();
        let mut reader = io::BufReader::with_capacity(64, f);
        let mut first = String::new();
        let a = reader.read_line(&mut first).norm();
        let mut rest = Vec::new();
        let b = reader.read_to_end(&mut rest).norm();
        let f = fs::File::open(file.path()).unwrap();
        let lines = io::BufReader::new(f).lines().count();
        let f = fs::File::open(file.path()).unwrap();
        let mut taken = String::new();
        let c = f.take(1000).read_to_string(&mut taken).norm();
        (a, first, b, rest, lines, c, taken)
    });
}
