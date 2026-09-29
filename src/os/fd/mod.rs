//! Owned and borrowed Unix-like file descriptors.
//!
//! The module needs the `io` feature.
//!
//! # Differences from std
//!
//! `Option<OwnedFd>` and `Option<BorrowedFd<'_>>` are twice the size of a
//! `RawFd`: std stores `None` as the invalid descriptor `-1`, a niche that
//! stable Rust cannot declare.

use core::{ffi::c_int, fmt, marker::PhantomData, mem::ManuallyDrop};

use alloc_crate::{boxed::Box, rc::Rc, sync::Arc};

#[cfg(feature = "fs")]
use crate::{fs, sys};
use crate::{
    io,
    sys::os::{close_fd, duplicate_fd},
};

// WASI has no sockets that litestd can make.
#[cfg(all(unix, feature = "net"))]
mod net;
mod pipe;

/// Raw file descriptors.
pub type RawFd = c_int;

/// A trait to extract the raw file descriptor from an underlying object.
pub trait AsRawFd {
    /// Extracts the raw file descriptor, which stays owned by `self`.
    fn as_raw_fd(&self) -> RawFd;
}

/// A trait to express the ability to construct an object from a raw file
/// descriptor.
pub trait FromRawFd {
    /// Constructs a new instance of `Self` from the given raw file
    /// descriptor, taking ownership of it.
    ///
    /// # Safety
    ///
    /// `fd` must be an open descriptor, owned by nothing else, that suits the
    /// type: the new object closes it when dropped.
    unsafe fn from_raw_fd(fd: RawFd) -> Self;
}

/// A trait to express the ability to consume an object and acquire ownership
/// of its raw file descriptor.
pub trait IntoRawFd {
    /// Consumes this object, returning the raw underlying file descriptor,
    /// which the caller must close.
    #[must_use = "losing the raw file descriptor may leak resources"]
    fn into_raw_fd(self) -> RawFd;
}

impl AsRawFd for RawFd {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        *self
    }
}

impl IntoRawFd for RawFd {
    #[inline]
    fn into_raw_fd(self) -> RawFd {
        self
    }
}

impl FromRawFd for RawFd {
    #[inline]
    unsafe fn from_raw_fd(fd: RawFd) -> Self {
        fd
    }
}

/// A borrowed file descriptor.
///
/// It has the representation of a `RawFd` and is never `-1`; nothing closes
/// the descriptor while the borrow lasts.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct BorrowedFd<'fd> {
    fd: RawFd,
    _phantom: PhantomData<&'fd OwnedFd>,
}

/// An owned file descriptor, closed when dropped.
///
/// It has the representation of a `RawFd` and is never `-1`. Errors from
/// closing are ignored, as in std.
#[repr(transparent)]
pub struct OwnedFd {
    fd: RawFd,
}

impl BorrowedFd<'_> {
    /// Returns a `BorrowedFd` holding the given raw file descriptor.
    ///
    /// # Safety
    ///
    /// The resource `fd` refers to must stay open for the lifetime of the
    /// returned `BorrowedFd`, and `fd` must not be `-1`.
    #[inline]
    #[allow(clippy::must_use_candidate, reason = "not `#[must_use]` in std")]
    pub const unsafe fn borrow_raw(fd: RawFd) -> Self {
        debug_assert!(fd != -1, "fd != -1");
        Self {
            fd,
            _phantom: PhantomData,
        }
    }

    /// Creates a new `OwnedFd` instance that shares the same underlying file
    /// description as the existing `BorrowedFd` instance. The new descriptor
    /// is close-on-exec and at least 3, so it never replaces a standard
    /// stream.
    ///
    /// # Errors
    ///
    /// Fails if the process has no descriptors left.
    pub fn try_clone_to_owned(&self) -> io::Result<OwnedFd> {
        let fd = duplicate_fd(self.fd)?;
        // SAFETY: `duplicate_fd` returned a new descriptor nothing else owns.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

impl OwnedFd {
    /// Creates a new `OwnedFd` instance that shares the same underlying file
    /// description as the existing `OwnedFd` instance.
    ///
    /// # Errors
    ///
    /// Fails if the process has no descriptors left.
    pub fn try_clone(&self) -> io::Result<Self> {
        self.as_fd().try_clone_to_owned()
    }
}

impl AsRawFd for BorrowedFd<'_> {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl AsRawFd for OwnedFd {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl IntoRawFd for OwnedFd {
    #[inline]
    fn into_raw_fd(self) -> RawFd {
        ManuallyDrop::new(self).fd
    }
}

impl FromRawFd for OwnedFd {
    /// Takes ownership of the raw file descriptor `fd`.
    ///
    /// # Safety
    ///
    /// `fd` must be open, owned by nothing else, and not `-1`.
    #[inline]
    unsafe fn from_raw_fd(fd: RawFd) -> Self {
        debug_assert!(fd != -1, "fd != -1");
        Self { fd }
    }
}

impl Drop for OwnedFd {
    #[inline]
    fn drop(&mut self) {
        close_fd(self.fd);
    }
}

impl fmt::Debug for BorrowedFd<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BorrowedFd").field("fd", &self.fd).finish()
    }
}

impl fmt::Debug for OwnedFd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedFd").field("fd", &self.fd).finish()
    }
}

/// A trait to borrow the file descriptor from an underlying object.
pub trait AsFd {
    /// Borrows the file descriptor.
    fn as_fd(&self) -> BorrowedFd<'_>;
}

impl<T: AsFd + ?Sized> AsFd for &T {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        T::as_fd(self)
    }
}

impl<T: AsFd + ?Sized> AsFd for &mut T {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        T::as_fd(self)
    }
}

impl AsFd for BorrowedFd<'_> {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        *self
    }
}

impl AsFd for OwnedFd {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        // SAFETY: an `OwnedFd` holds an open descriptor other than -1, and
        // the borrow cannot outlive `self`, which closes it only on drop.
        unsafe { BorrowedFd::borrow_raw(self.fd) }
    }
}

/// Implements `AsFd` and `AsRawFd` for smart pointers, through the pointee.
macro_rules! impl_fd_for_pointers {
    ($($ptr:ident)*) => {$(
        impl<T: AsFd + ?Sized> AsFd for $ptr<T> {
            #[inline]
            fn as_fd(&self) -> BorrowedFd<'_> {
                (**self).as_fd()
            }
        }

        impl<T: AsRawFd> AsRawFd for $ptr<T> {
            #[inline]
            fn as_raw_fd(&self) -> RawFd {
                (**self).as_raw_fd()
            }
        }
    )*};
}

impl_fd_for_pointers! { Arc Rc Box }

#[cfg(feature = "fs")]
impl AsFd for fs::File {
    #[inline]
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inner.as_fd()
    }
}

#[cfg(feature = "fs")]
impl From<fs::File> for OwnedFd {
    /// Takes ownership of a [`File`](fs::File)'s underlying file descriptor.
    #[inline]
    fn from(file: fs::File) -> Self {
        file.inner.into_fd()
    }
}

#[cfg(feature = "fs")]
impl From<OwnedFd> for fs::File {
    /// Returns a [`File`](fs::File) that takes ownership of `fd`.
    #[inline]
    fn from(fd: OwnedFd) -> Self {
        Self {
            inner: sys::fs::File::from_fd(fd),
        }
    }
}

#[cfg(feature = "fs")]
impl AsRawFd for fs::File {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_fd().as_raw_fd()
    }
}

#[cfg(feature = "fs")]
impl FromRawFd for fs::File {
    #[inline]
    unsafe fn from_raw_fd(fd: RawFd) -> Self {
        // SAFETY: the caller hands over ownership of an open descriptor.
        Self::from(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

#[cfg(feature = "fs")]
impl IntoRawFd for fs::File {
    #[inline]
    fn into_raw_fd(self) -> RawFd {
        OwnedFd::from(self).into_raw_fd()
    }
}
