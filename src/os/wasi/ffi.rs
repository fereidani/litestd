//! WASI-specific extensions to [`OsStr`] and [`OsString`], which on WASI
//! convert to and from bytes without copying.
//!
//! [`OsStr`]: crate::ffi::OsStr
//! [`OsString`]: crate::ffi::OsString

#[path = "../unix/ffi/os_str.rs"]
mod os_str;

pub use self::os_str::{OsStrExt, OsStringExt};
