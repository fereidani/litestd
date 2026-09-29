//! Unix-specific extensions.
//!
//! Each extension module comes with the feature of the module it extends,
//! and [`prelude`] re-exports the extension traits and handle types of the
//! enabled features.

#[cfg(feature = "path")]
pub mod ffi;
#[cfg(feature = "fs")]
pub mod fs;
#[cfg(feature = "io")]
pub mod io;
#[cfg(all(feature = "net", feature = "path"))]
pub mod net;
#[cfg(feature = "command")]
pub mod process;
#[cfg(feature = "thread")]
pub mod thread;

/// A prelude for conveniently writing platform-specific code.
pub mod prelude {
    #[cfg(feature = "path")]
    #[doc(no_inline)]
    pub use super::ffi::{OsStrExt, OsStringExt};
    #[cfg(feature = "fs")]
    #[doc(no_inline)]
    pub use super::fs::{
        DirEntryExt, FileExt, FileTypeExt, MetadataExt, OpenOptionsExt,
        PermissionsExt,
    };
    #[cfg(feature = "io")]
    #[doc(no_inline)]
    pub use super::io::{
        AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd,
    };
    #[cfg(feature = "command")]
    #[doc(no_inline)]
    pub use super::process::{CommandExt, ExitStatusExt};
    #[cfg(feature = "thread")]
    #[doc(no_inline)]
    pub use super::thread::JoinHandleExt;
}
