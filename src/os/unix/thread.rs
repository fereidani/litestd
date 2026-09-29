//! Unix-specific extensions to primitives in the [`thread`](crate::thread)
//! module.

#[cfg(target_os = "android")]
use core::ffi::c_long;
#[cfg(target_os = "linux")]
use core::ffi::c_ulong;

use crate::thread::JoinHandle;

/// A raw pthread id: std's integer type for `pthread_t` on this platform.
#[cfg(target_os = "linux")]
pub type RawPthread = c_ulong;

/// A raw pthread id: std's integer type for `pthread_t` on this platform.
#[cfg(target_os = "android")]
pub type RawPthread = c_long;

/// A raw pthread id: std's integer type for `pthread_t` on this platform.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub type RawPthread = usize;

/// Unix-specific extensions to [`JoinHandle`].
pub trait JoinHandleExt {
    /// Extracts the raw `pthread_t` without taking ownership.
    ///
    /// The id stays valid while the handle is alive.
    fn as_pthread_t(&self) -> RawPthread;

    /// Consumes the handle, returning the raw `pthread_t`.
    ///
    /// The caller becomes the thread's owner and must join or detach it. The
    /// closure's result is dropped here if the thread has finished, or else
    /// by the thread once it stores it.
    fn into_pthread_t(self) -> RawPthread;
}

impl<T> JoinHandleExt for JoinHandle<T> {
    fn as_pthread_t(&self) -> RawPthread {
        raw(self.as_native().as_raw())
    }

    fn into_pthread_t(self) -> RawPthread {
        raw(self.into_native().into_raw())
    }
}

/// Converts the C library's `pthread_t`, which is [`RawPthread`] already.
#[cfg(not(any(target_env = "musl", target_env = "ohos")))]
const fn raw(id: libc::pthread_t) -> RawPthread {
    id
}

/// Converts musl's `pthread_t`, a pointer to the thread's descriptor.
#[cfg(any(target_env = "musl", target_env = "ohos"))]
// `c_ulong` is as wide as a pointer on every Linux ABI: the cast is lossless.
#[allow(clippy::cast_possible_truncation)]
fn raw(id: libc::pthread_t) -> RawPthread {
    // Exposing the provenance, as std does, keeps the pointer usable for
    // code that casts the integer back to a `pthread_t`.
    id.expose_provenance() as RawPthread
}
