//! `litestd::ffi::c_str` against `std::ffi::c_str`: the same items, from
//! `core` and `alloc`, and the same ones as at `ffi`'s top level.

#![cfg(feature = "alloc")]

use litestd::ffi::{self, c_str};

#[test]
fn items_are_stds() {
    let _: fn(&c_str::CStr) -> &std::ffi::c_str::CStr = |s| s;
    let _: fn(c_str::CString) -> std::ffi::c_str::CString = |s| s;
    let _: fn(
        c_str::FromBytesUntilNulError,
    ) -> std::ffi::c_str::FromBytesUntilNulError = |e| e;
    let _: fn(
        c_str::FromBytesWithNulError,
    ) -> std::ffi::c_str::FromBytesWithNulError = |e| e;
    let _: fn(
        c_str::FromVecWithNulError,
    ) -> std::ffi::c_str::FromVecWithNulError = |e| e;
    let _: fn(c_str::IntoStringError) -> std::ffi::c_str::IntoStringError =
        |e| e;
    let _: fn(c_str::NulError) -> std::ffi::c_str::NulError = |e| e;
}

#[test]
fn items_are_the_top_level_ones() {
    let _: fn(&c_str::CStr) -> &ffi::CStr = |s| s;
    let _: fn(c_str::CString) -> ffi::CString = |s| s;
    let _: fn(c_str::FromBytesUntilNulError) -> ffi::FromBytesUntilNulError =
        |e| e;
    let _: fn(c_str::FromBytesWithNulError) -> ffi::FromBytesWithNulError =
        |e| e;
    let _: fn(c_str::FromVecWithNulError) -> ffi::FromVecWithNulError = |e| e;
    let _: fn(c_str::IntoStringError) -> ffi::IntoStringError = |e| e;
    let _: fn(c_str::NulError) -> ffi::NulError = |e| e;
}

#[test]
fn a_glob_import_brings_every_item() {
    use litestd::ffi::c_str::*;

    let owned = CString::new("c_str").unwrap();
    let borrowed: &CStr = &owned;
    assert_eq!(borrowed.to_bytes(), b"c_str");
    let _: NulError = CString::new("a\0b").unwrap_err();
    let _: FromBytesWithNulError = CStr::from_bytes_with_nul(b"a").unwrap_err();
    let _: FromBytesUntilNulError =
        CStr::from_bytes_until_nul(b"a").unwrap_err();
    let _: FromVecWithNulError =
        CString::from_vec_with_nul(b"a".to_vec()).unwrap_err();
    let _: IntoStringError =
        CString::new(vec![0xff]).unwrap().into_string().unwrap_err();
}
