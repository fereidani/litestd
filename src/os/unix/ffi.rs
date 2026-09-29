//! Unix-specific extensions to [`OsStr`] and [`OsString`], which on Unix
//! convert to and from bytes without copying.
//!
//! [`OsStr`]: crate::ffi::OsStr
//! [`OsString`]: crate::ffi::OsString

mod os_str;

pub use self::os_str::{OsStrExt, OsStringExt};
