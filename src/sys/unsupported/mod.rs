//! What a module without an OS service gets, as in std: operations fail
//! with `UNSUPPORTED`, and the handles they would create cannot exist.

// These stand in for OS calls with the other backends' signatures, which
// are not `const`; were they `const`, clippy would ask the portable callers
// to be too, unlike std's.
#![allow(
    clippy::missing_const_for_fn,
    clippy::needless_pass_by_ref_mut,
    clippy::unnecessary_wraps,
    clippy::unused_self
)]

/// Implements methods on `$ty`, whose field is uninhabited: no value can be
/// there to receive them, so each body is the empty match.
#[cfg(any(feature = "net", target_os = "unknown"))]
macro_rules! unreachable_methods {
    (
        $ty:ty;
        $(fn $name:ident(&self $(, $arg:ident: $arg_ty:ty)*) -> $ret:ty;)*
    ) => {
        impl $ty {
            $(
                pub(crate) fn $name(&self $(, $arg: $arg_ty)*) -> $ret {
                    $(let _ = $arg;)*
                    match self.0 {}
                }
            )*
        }
    };
}

#[cfg(all(target_os = "unknown", feature = "env"))]
pub(crate) mod env;
#[cfg(all(target_os = "unknown", feature = "fs"))]
pub(crate) mod fs;
#[cfg(feature = "net")]
pub(crate) mod net;
#[cfg(all(target_os = "unknown", feature = "io"))]
pub(crate) mod pipe;
#[cfg(feature = "command")]
pub(crate) mod process;

use crate::io;

/// std's error for an operation the platform lacks.
pub(crate) const UNSUPPORTED: io::Error = io::const_error!(
    io::ErrorKind::Unsupported,
    "operation not supported on this platform",
);
