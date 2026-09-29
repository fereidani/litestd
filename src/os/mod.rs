//! OS-specific functionality.
//!
//! [`raw`] is always available, the other modules on their platforms:
//! `darwin` on Apple's, `macos` besides it on macOS, `freebsd`, `netbsd`,
//! `openbsd` and `dragonfly` on theirs, `fd` on WASI, and `wasi` on WASI
//! 0.1.
//! Each extension module comes with the features of the modules it extends.

pub mod raw {
    //! Compatibility aliases for the C types of [`core::ffi`].

    pub use core::ffi::{
        c_char, c_double, c_float, c_int, c_long, c_longlong, c_schar, c_short,
        c_uchar, c_uint, c_ulong, c_ulonglong, c_ushort, c_void,
    };
}

/// Declares the module of a BSD, whose `fs` extension is the one shared
/// `MetadataExt`; `title` is the BSD's name in the documentation.
#[cfg(all(
    any(
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ),
    feature = "fs"
))]
macro_rules! bsd_module {
    ($name:ident, $title:literal) => {
        #[doc = concat!($title, "-specific definitions.")]
        pub mod $name {
            #[doc = concat!($title, "-specific extensions to primitives in")]
            #[doc = "the [`fs`](crate::fs) module."]
            pub mod fs {
                pub use crate::os::bsd_fs::MetadataExt;
            }
        }
    };
}

#[cfg(all(target_os = "android", feature = "net"))]
pub mod android;
#[cfg(all(
    any(
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ),
    feature = "fs"
))]
mod bsd_fs;
#[cfg(all(target_vendor = "apple", feature = "fs"))]
pub mod darwin;
#[cfg(all(target_os = "dragonfly", feature = "fs"))]
bsd_module!(dragonfly, "DragonFly BSD");
#[cfg(all(any(unix, target_os = "wasi"), feature = "io"))]
pub mod fd;
#[cfg(all(target_os = "freebsd", feature = "fs"))]
bsd_module!(freebsd, "FreeBSD");
#[cfg(all(target_os = "linux", any(feature = "fs", feature = "net")))]
pub mod linux;
#[cfg(all(target_os = "macos", feature = "fs"))]
pub mod macos;
#[cfg(all(target_os = "netbsd", feature = "fs"))]
bsd_module!(netbsd, "NetBSD");
#[cfg(all(target_os = "openbsd", feature = "fs"))]
bsd_module!(openbsd, "OpenBSD");
#[cfg(unix)]
pub mod unix;
// std's `os::wasi` is unstable on WASI 0.2.
#[cfg(all(target_os = "wasi", target_env = "p1"))]
pub mod wasi;
#[cfg(windows)]
pub mod windows;
