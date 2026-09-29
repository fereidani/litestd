//! Windows-specific extensions to general I/O primitives.
//!
//! Null and `INVALID_HANDLE_VALUE` are both valid handle values, so
//! [`HandleOrNull`] and [`HandleOrInvalid`] check a Win32 return value for
//! the failure sentinel of its function before it becomes an [`OwnedHandle`].

use core::{error::Error, ffi::c_void, fmt, marker::PhantomData, mem, ptr};

use alloc_crate::{boxed::Box, rc::Rc, sync::Arc};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle,
        INVALID_HANDLE_VALUE,
    },
    System::Threading::GetCurrentProcess,
};

#[cfg(feature = "net")]
pub use self::socket::*;
use crate::io;
#[cfg(feature = "fs")]
use crate::{fs, sys};

/// Implements a trait that borrows a handle or socket for references, `Arc`,
/// `Rc` and `Box` by forwarding to `T`; the bracketed tokens extend the bound
/// on `T`.
macro_rules! forward_borrow {
    ($trait:ident, $method:ident, $borrowed:ident, [$($bound:tt)*]) => {
        forward_borrow!(@one $trait, $method, $borrowed, [$($bound)*], &T);
        forward_borrow!(@one $trait, $method, $borrowed, [$($bound)*], &mut T);
        forward_borrow!(@one $trait, $method, $borrowed, [$($bound)*], Rc<T>);
        forward_borrow!(@one $trait, $method, $borrowed, [$($bound)*], Box<T>);

        #[doc = concat!(
            "This impl allows implementing traits that require `",
            stringify!($trait), "` on `Arc`."
        )]
        impl<T: $trait $($bound)*> $trait for Arc<T> {
            #[inline]
            fn $method(&self) -> $borrowed<'_> {
                (**self).$method()
            }
        }
    };
    (
        @one $trait:ident, $method:ident, $borrowed:ident,
        [$($bound:tt)*], $ptr:ty
    ) => {
        impl<T: $trait $($bound)*> $trait for $ptr {
            #[inline]
            fn $method(&self) -> $borrowed<'_> {
                (**self).$method()
            }
        }
    };
}

mod pipe;
#[cfg(feature = "net")]
mod socket;

/// Raw `HANDLE`s.
pub type RawHandle = *mut c_void;

/// Extracts raw handles.
pub trait AsRawHandle {
    /// Extracts the raw handle, without passing ownership. It may be null,
    /// such as for the stdio of a process without a console.
    fn as_raw_handle(&self) -> RawHandle;
}

/// Constructs I/O objects from raw handles.
pub trait FromRawHandle {
    /// Constructs a new I/O object that takes ownership of `handle`.
    ///
    /// # Safety
    ///
    /// `handle` must be open and not owned or borrowed by anything else; null
    /// and `INVALID_HANDLE_VALUE` are valid values.
    unsafe fn from_raw_handle(handle: RawHandle) -> Self;
}

/// Consumes an object, handing over ownership of its raw `HANDLE`.
pub trait IntoRawHandle {
    /// Consumes this object, returning the raw handle for the caller to close.
    #[must_use = "losing the raw handle may leak resources"]
    fn into_raw_handle(self) -> RawHandle;
}

/// A borrowed handle, tied to the lifetime of its owner. It is
/// `repr(transparent)` over a raw handle, and may be null or
/// `INVALID_HANDLE_VALUE` (see [the module documentation](self)).
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct BorrowedHandle<'handle> {
    handle: RawHandle,
    _phantom: PhantomData<&'handle OwnedHandle>,
}

/// An owned handle, closed on drop with `CloseHandle`, so it must not hold a
/// registry key. It may be null or `INVALID_HANDLE_VALUE` (see [the module
/// documentation](self)).
#[repr(transparent)]
pub struct OwnedHandle {
    handle: RawHandle,
}

/// FFI type for handles from functions such as `CreateThread` that signal
/// failure with null. [`TryFrom`] converts it to an [`OwnedHandle`].
///
/// `-1` is a valid value here, such as the current process's pseudo handle.
/// A non-null handle is closed on drop.
#[derive(Debug)]
#[repr(transparent)]
pub struct HandleOrNull(RawHandle);

/// FFI type for handles from functions such as `CreateFileW` that signal
/// failure with `INVALID_HANDLE_VALUE`. [`TryFrom`] converts it to an
/// [`OwnedHandle`]. Any other handle is closed on drop.
#[derive(Debug)]
#[repr(transparent)]
pub struct HandleOrInvalid(RawHandle);

// SAFETY: a handle indexes the process-wide kernel handle table; any thread
// may use or close it.
unsafe impl Send for OwnedHandle {}
// SAFETY: see `Send for OwnedHandle`.
unsafe impl Send for HandleOrNull {}
// SAFETY: see `Send for OwnedHandle`.
unsafe impl Send for HandleOrInvalid {}
// SAFETY: see `Send for OwnedHandle`.
unsafe impl Send for BorrowedHandle<'_> {}
// SAFETY: as for `Send`; the kernel synchronizes operations on one handle.
unsafe impl Sync for OwnedHandle {}
// SAFETY: see `Sync for OwnedHandle`.
unsafe impl Sync for HandleOrNull {}
// SAFETY: see `Sync for OwnedHandle`.
unsafe impl Sync for HandleOrInvalid {}
// SAFETY: see `Sync for OwnedHandle`.
unsafe impl Sync for BorrowedHandle<'_> {}

impl BorrowedHandle<'_> {
    /// Returns a `BorrowedHandle` holding the given raw handle.
    ///
    /// # Safety
    ///
    /// `handle` must stay open for the lifetime of the result; null and
    /// `INVALID_HANDLE_VALUE` are valid values.
    #[inline]
    #[must_use]
    pub const unsafe fn borrow_raw(handle: RawHandle) -> Self {
        Self {
            handle,
            _phantom: PhantomData,
        }
    }

    /// Creates a new `OwnedHandle` for the same object as this one.
    ///
    /// # Errors
    ///
    /// Returns the error of `DuplicateHandle`.
    pub fn try_clone_to_owned(&self) -> io::Result<OwnedHandle> {
        // Stdio handles can be null in a process without a console. Null
        // does no I/O and cannot be duplicated, so its clone is null too.
        if self.handle.is_null() {
            return Ok(OwnedHandle {
                handle: ptr::null_mut(),
            });
        }
        duplicate(self.handle, false)
    }
}

impl OwnedHandle {
    /// Creates a new `OwnedHandle` for the same object as this one.
    ///
    /// # Errors
    ///
    /// Returns the error of `DuplicateHandle`.
    pub fn try_clone(&self) -> io::Result<Self> {
        self.as_handle().try_clone_to_owned()
    }
}

/// Duplicates `handle`, which must be open, in this process with the same
/// access, inheritable by child processes if `inheritable`.
pub(crate) fn duplicate(
    handle: RawHandle,
    inheritable: bool,
) -> io::Result<OwnedHandle> {
    let mut copy = ptr::null_mut();
    // SAFETY: the pseudo handle of `GetCurrentProcess` needs no closing, the
    // callers own or borrow `handle`, and `copy` is writable.
    let ok = unsafe {
        let process = GetCurrentProcess();
        DuplicateHandle(
            process,
            handle,
            process,
            &raw mut copy,
            0,
            i32::from(inheritable),
            DUPLICATE_SAME_ACCESS,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(OwnedHandle { handle: copy })
}

impl TryFrom<HandleOrNull> for OwnedHandle {
    type Error = NullHandleError;

    #[inline]
    fn try_from(handle_or_null: HandleOrNull) -> Result<Self, NullHandleError> {
        let handle = mem::ManuallyDrop::new(handle_or_null).0;
        if handle.is_null() {
            Err(NullHandleError(()))
        } else {
            Ok(Self { handle })
        }
    }
}

impl TryFrom<HandleOrInvalid> for OwnedHandle {
    type Error = InvalidHandleError;

    #[inline]
    fn try_from(
        handle_or_invalid: HandleOrInvalid,
    ) -> Result<Self, InvalidHandleError> {
        let handle = mem::ManuallyDrop::new(handle_or_invalid).0;
        if handle == INVALID_HANDLE_VALUE {
            Err(InvalidHandleError(()))
        } else {
            Ok(Self { handle })
        }
    }
}

impl HandleOrNull {
    /// Wraps a handle returned by a Windows API that signals failure with
    /// null, such as `CreateThread`.
    ///
    /// # Safety
    ///
    /// `handle` must be null or satisfy [`FromRawHandle::from_raw_handle`].
    #[inline]
    #[must_use]
    pub const unsafe fn from_raw_handle(handle: RawHandle) -> Self {
        Self(handle)
    }
}

impl HandleOrInvalid {
    /// Wraps a handle returned by a Windows API that signals failure with
    /// `INVALID_HANDLE_VALUE`, such as `CreateFileW`.
    ///
    /// # Safety
    ///
    /// `handle` must be `INVALID_HANDLE_VALUE` or satisfy
    /// [`FromRawHandle::from_raw_handle`].
    #[inline]
    #[must_use]
    pub const unsafe fn from_raw_handle(handle: RawHandle) -> Self {
        Self(handle)
    }
}

/// Closes a handle that the caller owns.
fn close(handle: RawHandle) {
    // SAFETY: every caller owns `handle` and gives it up here. Closing an
    // owned, open handle cannot fail, so the result is ignored, as in std.
    unsafe { CloseHandle(handle) };
}

impl Drop for OwnedHandle {
    #[inline]
    fn drop(&mut self) {
        close(self.handle);
    }
}

impl Drop for HandleOrNull {
    #[inline]
    fn drop(&mut self) {
        if !self.0.is_null() {
            close(self.0);
        }
    }
}

impl Drop for HandleOrInvalid {
    #[inline]
    fn drop(&mut self) {
        if self.0 != INVALID_HANDLE_VALUE {
            close(self.0);
        }
    }
}

/// The error of converting a null [`HandleOrNull`] to a handle.
// The private field prevents construction outside this module.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NullHandleError(());

impl fmt::Display for NullHandleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(
            "A HandleOrNull could not be converted to a handle because it \
             was null",
            f,
        )
    }
}

impl Error for NullHandleError {}

/// The error of converting an invalid [`HandleOrInvalid`] to a handle.
// The private field prevents construction outside this module.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvalidHandleError(());

impl fmt::Display for InvalidHandleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(
            "A HandleOrInvalid could not be converted to a handle because \
             it was INVALID_HANDLE_VALUE",
            f,
        )
    }
}

impl Error for InvalidHandleError {}

impl AsRawHandle for BorrowedHandle<'_> {
    #[inline]
    fn as_raw_handle(&self) -> RawHandle {
        self.handle
    }
}

impl AsRawHandle for OwnedHandle {
    #[inline]
    fn as_raw_handle(&self) -> RawHandle {
        self.handle
    }
}

impl IntoRawHandle for OwnedHandle {
    #[inline]
    fn into_raw_handle(self) -> RawHandle {
        mem::ManuallyDrop::new(self).handle
    }
}

impl FromRawHandle for OwnedHandle {
    #[inline]
    unsafe fn from_raw_handle(handle: RawHandle) -> Self {
        Self { handle }
    }
}

impl fmt::Debug for BorrowedHandle<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BorrowedHandle")
            .field("handle", &self.handle)
            .finish()
    }
}

impl fmt::Debug for OwnedHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedHandle")
            .field("handle", &self.handle)
            .finish()
    }
}

/// A trait to borrow the handle from an underlying object.
pub trait AsHandle {
    /// Borrows the handle.
    fn as_handle(&self) -> BorrowedHandle<'_>;
}

forward_borrow!(AsHandle, as_handle, BorrowedHandle, [+ ?Sized]);

impl AsHandle for BorrowedHandle<'_> {
    #[inline]
    fn as_handle(&self) -> BorrowedHandle<'_> {
        *self
    }
}

impl AsHandle for OwnedHandle {
    #[inline]
    fn as_handle(&self) -> BorrowedHandle<'_> {
        // SAFETY: `self` owns the handle, which stays open while the
        // returned borrow, tied to `&self`, lives.
        unsafe { BorrowedHandle::borrow_raw(self.handle) }
    }
}

#[cfg(feature = "fs")]
impl AsHandle for fs::File {
    #[inline]
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.inner.handle().as_handle()
    }
}

#[cfg(feature = "fs")]
impl From<fs::File> for OwnedHandle {
    /// Takes ownership of a [`File`](fs::File)'s underlying file handle.
    #[inline]
    fn from(file: fs::File) -> Self {
        file.inner.into_handle()
    }
}

#[cfg(feature = "fs")]
impl From<OwnedHandle> for fs::File {
    /// Returns a [`File`](fs::File) that takes ownership of `handle`.
    #[inline]
    fn from(handle: OwnedHandle) -> Self {
        Self {
            inner: sys::fs::File::from_handle(handle),
        }
    }
}

#[cfg(feature = "fs")]
impl AsRawHandle for fs::File {
    #[inline]
    fn as_raw_handle(&self) -> RawHandle {
        self.inner.handle().as_raw_handle()
    }
}

#[cfg(feature = "fs")]
impl FromRawHandle for fs::File {
    #[inline]
    unsafe fn from_raw_handle(handle: RawHandle) -> Self {
        // SAFETY: the caller passes an owned handle, as `OwnedHandle` requires.
        Self::from(unsafe { OwnedHandle::from_raw_handle(handle) })
    }
}

#[cfg(feature = "fs")]
impl IntoRawHandle for fs::File {
    #[inline]
    fn into_raw_handle(self) -> RawHandle {
        self.inner.into_handle().into_raw_handle()
    }
}
