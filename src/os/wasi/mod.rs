//! WASI-specific definitions.
//!
//! Each extension module comes with the feature of the module it extends,
//! and [`prelude`] re-exports the extension traits and descriptor types of
//! the enabled features.

#[cfg(feature = "path")]
pub mod ffi;
#[cfg(feature = "io")]
pub mod io;

/// A prelude for conveniently writing platform-specific code.
pub mod prelude {
    #[cfg(feature = "path")]
    #[doc(no_inline)]
    pub use super::ffi::{OsStrExt, OsStringExt};
    #[cfg(feature = "io")]
    #[doc(no_inline)]
    pub use super::io::{
        AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd,
    };
}
