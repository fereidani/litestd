//! Platform-specific types, as defined by C, and C string types.
//!
//! With the `path` feature this module also provides [`OsStr`] and
//! [`OsString`], the platform strings that paths are built on.

pub use core::ffi::*;

#[cfg(feature = "alloc")]
pub use self::c_str::{
    CString, FromVecWithNulError, IntoStringError, NulError,
};

// Shadows the glob's `c_str`, which has only `core`'s part, on toolchains
// that export it.
pub mod c_str {
    //! [`CStr`], `CString`, and related types.
    //!
    //! `CString` and its errors come with the `alloc` feature.

    pub use core::ffi::{CStr, FromBytesUntilNulError, FromBytesWithNulError};

    #[cfg(feature = "alloc")]
    pub use alloc_crate::ffi::{
        CString, FromVecWithNulError, IntoStringError, NulError,
    };
}

#[cfg(feature = "path")]
pub mod os_str;

#[cfg(feature = "path")]
pub use self::os_str::{OsStr, OsString};

#[cfg(any(feature = "io", feature = "path"))]
mod text;

#[cfg(feature = "path")]
pub(crate) use self::text::fmt_lossy;
#[cfg(feature = "io")]
pub(crate) use self::text::pad;
