//! litestd's `io::Error` against std's: returning it through calls,
//! inspecting it, creating and dropping it, and a real failing call.

#![allow(clippy::unwrap_used, reason = "a failure ends the benchmark")]

mod common;

use core::{error::Error, fmt, hint::black_box};

use common::compare;

/// `ENOENT` on Unix and `ERROR_FILE_NOT_FOUND` on Windows.
const NOT_FOUND: i32 = 2;

/// Defines `$name`, the outermost of a chain of calls that return
/// `io::Result<()>` of crate `$krate`, the innermost failing with an OS
/// error if asked.
macro_rules! chain {
    ($name:ident, $krate:ident) => {
        #[inline(never)]
        fn $name(fail: bool) -> $krate::io::Result<()> {
            #[inline(never)]
            fn leaf(fail: bool) -> $krate::io::Result<()> {
                if fail {
                    Err($krate::io::Error::from_raw_os_error(NOT_FOUND))
                } else {
                    Ok(())
                }
            }

            #[inline(never)]
            fn middle(fail: bool) -> $krate::io::Result<()> {
                leaf(fail)?;
                leaf(false)
            }

            middle(fail)?;
            middle(false)
        }
    };
}

chain!(lite_chain, litestd);
chain!(std_chain, std);

/// A small payload for custom errors.
#[derive(Debug)]
struct Payload(u32);

impl fmt::Display for Payload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "payload {}", self.0)
    }
}

impl Error for Payload {}

fn main() {
    for fail in [false, true] {
        let name = if fail {
            "io Result<()> Err through 3 calls"
        } else {
            "io Result<()> Ok through 3 calls"
        };
        compare(
            name,
            || drop(black_box(lite_chain(black_box(fail)))),
            || drop(black_box(std_chain(black_box(fail)))),
        );
    }

    let lite = litestd::io::Error::from_raw_os_error(NOT_FOUND);
    let std = std::io::Error::from_raw_os_error(NOT_FOUND);
    compare(
        "io Error::kind of an OS error",
        || {
            black_box(black_box(&lite).kind());
        },
        || {
            black_box(black_box(&std).kind());
        },
    );
    compare(
        "io Error::last_os_error",
        || drop(black_box(litestd::io::Error::last_os_error())),
        || drop(black_box(std::io::Error::last_os_error())),
    );
    compare(
        "io Error::new and drop",
        || {
            let payload = Payload(black_box(7));
            let kind = litestd::io::ErrorKind::InvalidData;
            drop(black_box(litestd::io::Error::new(kind, payload)));
        },
        || {
            let payload = Payload(black_box(7));
            let kind = std::io::ErrorKind::InvalidData;
            drop(black_box(std::io::Error::new(kind, payload)));
        },
    );

    // Never created, so opening it fails with `NotFound`.
    let name = format!("litestd-bench-io-missing-{}", std::process::id());
    let missing = std::env::temp_dir().join(name);
    let missing = missing.into_os_string().into_string().unwrap();
    compare(
        "io open a missing file",
        || drop(black_box(litestd::fs::File::open(black_box(&*missing)))),
        || drop(black_box(std::fs::File::open(black_box(&missing)))),
    );
}
