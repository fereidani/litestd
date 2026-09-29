//! The facade exposes `core` and `alloc` at the paths std uses, and std's
//! macros at the crate root.

#![cfg(all(feature = "alloc", feature = "stdio"))]

use litestd::{
    collections::VecDeque, ffi::CString, fmt::Write as _,
    prelude::rust_2024::*, sync::Arc, time::Duration,
};

#[test]
fn std_paths_resolve_to_core_and_alloc() {
    let bytes: Vec<u8> = litestd::vec![1, 2, 3];
    let text: String = litestd::format!("{}-{}", bytes.len(), "x");
    assert_eq!(text, "3-x");
    let queue = VecDeque::from(bytes);
    assert_eq!(queue.len(), 3);
    let c_string = CString::new("abc").unwrap();
    assert_eq!(c_string.as_bytes().len(), 3);
    let shared = Arc::new(Duration::from_millis(5));
    assert_eq!(*shared, Duration::from_millis(5));
    let boxed: Box<str> = "boxed".to_string().into_boxed_str();
    assert_eq!(&*boxed, "boxed");
}

#[test]
fn std_macros_resolve_through_the_crate() {
    let mut out = String::new();
    litestd::write!(out, "{}", 1).unwrap();
    litestd::writeln!(out, "{}", 2).unwrap();
    assert_eq!(out, "12\n");
    litestd::assert_eq!(litestd::format!("{:?}", litestd::vec![1u8]), "[1]");
    litestd::assert!(litestd::matches!(Some(3), Some(3)));
    litestd::debug_assert_ne!(1, 2);
    assert_eq!(litestd::stringify!(a + b), "a + b");
    assert_eq!(litestd::concat!("a", 1), "a1");
    assert!(litestd::file!().ends_with("facade.rs"));
    assert!(litestd::module_path!().starts_with("facade"));
    assert!(litestd::option_env!("LITESTD_NOT_SET").is_none());
    let value = litestd::dbg!(21) * 2;
    assert_eq!(value, 42);
}

/// What std re-exports from `core` of Rust 1.95 and 1.96, which litestd
/// provides with those compilers too. These tests need one of them.
#[test]
fn newer_core_items_resolve_through_the_crate() {
    litestd::assert_matches!(Some(3), Some(x) if x > 2);
    litestd::debug_assert_matches!(Ok::<u8, ()>(1), Ok(_));
    let range = litestd::range::Range { start: 1u8, end: 3 };
    assert_eq!((range.start, range.end), (1, 3));
    let family = litestd::cfg_select! {
        unix => "unix",
        windows => "windows",
        _ => "",
    };
    assert_eq!(family, std::env::consts::FAMILY);
}

#[test]
fn print_macros_accept_std_syntax() {
    // The output goes to the real stdout and stderr; tests/stdio.rs checks
    // it byte for byte.
    let name = "facade";
    litestd::print!("");
    litestd::println!();
    litestd::println!("{name} {} {:>3}", 1, 2);
    litestd::eprint!("{}", "");
    litestd::eprintln!("{name:?}",);
}
