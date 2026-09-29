//! A `no_std` program written against std's API.
//!
//! The only change from a std program is the switch at the top: the crate
//! imports litestd under the name `std`, and every `std::` path and std
//! macro below resolves through it.
//!
//! Build with `--features global-allocator,panic-location`. Without
//! arguments it runs its checks and prints a report. The argument `panic`
//! panics, which prints the panic location and aborts; `exit` exits with
//! code 3, and `abort` aborts.

#![no_std]
#![no_main]
// Written against std's paths on purpose: that is what the switch provides.
#![allow(clippy::std_instead_of_core, clippy::std_instead_of_alloc)]
// A std-style program: its checks panic on failure, like tests, and one
// mode panics on purpose.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[macro_use]
extern crate litestd as std;

use std::{
    borrow::Cow,
    collections::BTreeMap,
    ffi::{CStr, c_char, c_int},
    fmt::Write as _,
    io,
    prelude::rust_2024::*,
    process,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// The C runtime's entry point, which wasi-libc calls `__main_argc_argv`.
#[cfg_attr(not(target_os = "wasi"), unsafe(no_mangle))]
#[cfg_attr(target_os = "wasi", unsafe(export_name = "__main_argc_argv"))]
#[allow(clippy::similar_names, reason = "the C names")]
extern "C" fn main(argc: c_int, argv: *const *const c_char) -> c_int {
    let count = usize::try_from(argc).unwrap_or(0);
    let arguments: Vec<&CStr> = (0..count)
        // SAFETY: the C runtime passes `argc` valid, NUL-terminated
        // strings that live until the process exits.
        .map(|i| unsafe { CStr::from_ptr(*argv.add(i)) })
        .collect();
    match arguments.get(1).map(|arg| arg.to_bytes()) {
        Some(b"panic") => panic!("deliberate panic with {count} arguments"),
        Some(b"exit") => process::exit(3),
        Some(b"abort") => process::abort(),
        _ => {}
    }
    collections();
    // wasm32-unknown-unknown has no clock, and WebAssembly no process ids:
    // `Instant::now` and `process::id` panic there, as in std.
    if cfg!(not(target_os = "unknown")) {
        time();
    }
    errors();
    if cfg!(target_family = "wasm") {
        println!("ok");
    } else {
        println!("pid {} ok", process::id());
    }
    0
}

fn collections() {
    let v = vec![1u32, 2, 3];
    let total: u32 = v.iter().sum();
    assert_eq!(total, 6);

    let mut s = String::new();
    let suffix = "x";
    write!(s, "{total}-{suffix}").unwrap();
    assert_eq!(s, format!("{}-x", 6));
    assert_eq!(std::format!("{s}!"), "6-x!");

    let mut map = BTreeMap::new();
    map.insert("b", 2);
    map.insert("a", 1);
    assert_eq!(map.keys().copied().collect::<Vec<_>>(), ["a", "b"]);

    let shared = Arc::new(Box::new(5));
    let local = Rc::new(Cow::Borrowed("borrowed"));
    assert_eq!(**shared + local.len(), 13);

    let doubled = dbg!(total * 2);
    assert_eq!(doubled, 12);
    println!("collections: {v:?} {s} {map:?}");
}

fn time() {
    let start = Instant::now();
    let later = start + Duration::from_millis(1);
    assert!(later > start);
    assert_eq!(later - start, Duration::from_millis(1));
    assert!(start.checked_sub(Duration::from_secs(1 << 40)).is_some());

    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is set after 1970");
    assert!(since_epoch > Duration::from_secs(1_600_000_000));
    let before_1970 = UNIX_EPOCH - Duration::from_secs(1);
    let err = before_1970.duration_since(UNIX_EPOCH).unwrap_err();
    assert_eq!(err.duration(), Duration::from_secs(1));

    std::eprintln!("elapsed {:?}", start.elapsed());
    std::println!("time: {} s since the epoch", since_epoch.as_secs());
}

fn errors() {
    let not_found = io::Error::from(io::ErrorKind::NotFound);
    assert_eq!(not_found.kind(), io::ErrorKind::NotFound);
    let custom = io::Error::other("custom failure");
    assert_eq!(custom.to_string(), "custom failure");
    let os = io::Error::from_raw_os_error(2);
    assert_eq!(os.raw_os_error(), Some(2));
    println!("errors: {os}");
}
