//! Windows-specific extensions.
//!
//! [`raw`] is always available; each other module needs the feature of the
//! module it extends.

#[cfg(feature = "path")]
pub mod ffi;
#[cfg(feature = "fs")]
pub mod fs;
#[cfg(feature = "io")]
pub mod io;
#[cfg(feature = "command")]
pub mod process;

pub mod raw {
    //! Windows-specific primitives.

    use core::ffi::c_void;

    /// A raw handle to a kernel object.
    pub type HANDLE = *mut c_void;

    /// A raw socket.
    #[cfg(target_pointer_width = "32")]
    pub type SOCKET = u32;

    /// A raw socket.
    #[cfg(target_pointer_width = "64")]
    pub type SOCKET = u64;
}

/// Windows-specific extensions to the [`thread`](crate::thread) module:
/// [`JoinHandle`](crate::thread::JoinHandle) implements
/// [`AsRawHandle`](io::AsRawHandle) and [`IntoRawHandle`](io::IntoRawHandle).
#[cfg(feature = "thread")]
pub mod thread {}

/// A prelude for conveniently writing platform-specific code: the extension
/// traits and handle types of the enabled features.
pub mod prelude {
    #[cfg(feature = "path")]
    #[doc(no_inline)]
    pub use super::ffi::{OsStrExt, OsStringExt};
    #[cfg(feature = "fs")]
    #[doc(no_inline)]
    pub use super::fs::{FileExt, MetadataExt, OpenOptionsExt};
    #[cfg(feature = "io")]
    #[doc(no_inline)]
    pub use super::io::{
        AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, HandleOrInvalid,
        IntoRawHandle, OwnedHandle, RawHandle,
    };
    #[cfg(feature = "net")]
    #[doc(no_inline)]
    pub use super::io::{
        AsRawSocket, AsSocket, BorrowedSocket, FromRawSocket, IntoRawSocket,
        OwnedSocket, RawSocket,
    };
}
