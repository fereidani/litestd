//! [`IoSlice`] and [`IoSliceMut`], the buffers of vectored I/O: over
//! `libc::iovec` on Unix and WASI and Winsock's `WSABUF` on Windows, so that a
//! slice of them can be passed to the OS directly, as std guarantees, and a
//! pointer and a length elsewhere.

use core::{
    fmt,
    marker::PhantomData,
    mem,
    ops::{Deref, DerefMut},
    slice,
};

use self::raw::Raw;

/// A buffer type used with
/// [`Write::write_vectored`](super::Write::write_vectored): a `&[u8]` that is
/// ABI compatible with `iovec` on Unix and `WSABUF` on Windows.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct IoSlice<'a> {
    /// The pointer and length of the `&'a [u8]` given to `new`, the only
    /// constructor; nothing writes the field afterwards.
    raw: Raw,
    marker: PhantomData<&'a [u8]>,
}

/// A buffer type used with
/// [`Read::read_vectored`](super::Read::read_vectored): a `&mut [u8]` that is
/// ABI compatible with `iovec` on Unix and `WSABUF` on Windows.
#[repr(transparent)]
pub struct IoSliceMut<'a> {
    /// The pointer and length of the `&'a mut [u8]` given to `new`, the only
    /// constructor; nothing writes the field afterwards.
    raw: Raw,
    marker: PhantomData<&'a mut [u8]>,
}

// SAFETY: an `IoSlice` is a `&[u8]`, which is `Send` and `Sync`.
unsafe impl Send for IoSlice<'_> {}
// SAFETY: as above.
unsafe impl Sync for IoSlice<'_> {}

// SAFETY: an `IoSliceMut` is a `&mut [u8]`, which is `Send` and `Sync`.
unsafe impl Send for IoSliceMut<'_> {}
// SAFETY: as above.
unsafe impl Sync for IoSliceMut<'_> {}

impl<'a> IoSlice<'a> {
    /// Creates a new `IoSlice` wrapping a byte slice.
    ///
    /// # Panics
    ///
    /// Panics on Windows if the slice is larger than 4GB.
    #[must_use]
    #[inline]
    #[track_caller]
    #[allow(clippy::missing_const_for_fn, reason = "not `const` in std")]
    pub fn new(buf: &'a [u8]) -> Self {
        Self {
            // The pointer is mutable only in the C declaration; nothing
            // writes through it.
            raw: Raw::new(buf.as_ptr().cast_mut(), buf.len()),
            marker: PhantomData,
        }
    }

    /// The bytes of the slice given to `new`.
    #[inline]
    const fn as_slice(&self) -> &'a [u8] {
        // SAFETY: by the invariant of `raw`, its pointer and length are those
        // of a `&'a [u8]`: non-null, aligned, valid for reads of `len` bytes
        // for `'a`, and at most `isize::MAX` bytes long.
        unsafe {
            slice::from_raw_parts(self.raw.ptr().cast_const(), self.raw.len())
        }
    }

    /// Advances the internal cursor of the slice by `n` bytes.
    ///
    /// # Panics
    ///
    /// Panics when trying to advance beyond the end of the slice.
    #[inline]
    #[track_caller]
    pub fn advance(&mut self, n: usize) {
        match self.as_slice().get(n..) {
            Some(rest) => *self = Self::new(rest),
            None => slice_overrun(),
        }
    }

    /// Advances a slice of slices by `n` bytes, removing the buffers, empty
    /// ones included, that are fully advanced over.
    ///
    /// # Panics
    ///
    /// Panics when trying to advance beyond the end of the slices.
    #[inline]
    #[track_caller]
    #[allow(clippy::mut_mut, reason = "std's signature")]
    pub fn advance_slices(bufs: &mut &mut [Self], n: usize) {
        let (remove, left) = consumed(bufs.iter().map(|b| b.len()), n);
        *bufs = mem::take(bufs).get_mut(remove..).unwrap_or_default();
        match bufs.first_mut() {
            Some(first) => first.advance(left),
            None if left == 0 => {}
            None => slices_overrun(),
        }
    }
}

impl<'a> IoSliceMut<'a> {
    /// Creates a new `IoSliceMut` wrapping a byte slice.
    ///
    /// # Panics
    ///
    /// Panics on Windows if the slice is larger than 4GB.
    #[inline]
    #[track_caller]
    #[allow(clippy::missing_const_for_fn, reason = "not `const` in std")]
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self {
            raw: Raw::new(buf.as_mut_ptr(), buf.len()),
            marker: PhantomData,
        }
    }

    /// The bytes of the slice given to `new`.
    #[inline]
    const fn as_slice(&self) -> &[u8] {
        // SAFETY: by the invariant of `raw`, its pointer and length are those
        // of a `&'a mut [u8]`: non-null, aligned, valid for reads and writes
        // of `len` bytes for `'a`, and at most `isize::MAX` bytes long. `self`
        // holds that exclusive borrow, and the result borrows `self`, so
        // nothing writes the bytes while the result lives.
        unsafe {
            slice::from_raw_parts(self.raw.ptr().cast_const(), self.raw.len())
        }
    }

    /// The bytes of the slice given to `new`, for writing.
    #[inline]
    const fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: as in `as_slice`; the result borrows `self` mutably, so it
        // is the only way to reach the bytes while it lives.
        unsafe { slice::from_raw_parts_mut(self.raw.ptr(), self.raw.len()) }
    }

    /// The bytes of the slice given to `new`, for as long as it lives.
    #[inline]
    const fn into_slice(self) -> &'a mut [u8] {
        // SAFETY: as in `as_slice`; `self` is consumed, so the result takes
        // over its exclusive borrow for `'a`.
        unsafe { slice::from_raw_parts_mut(self.raw.ptr(), self.raw.len()) }
    }

    /// Advances the internal cursor of the slice by `n` bytes.
    ///
    /// # Panics
    ///
    /// Panics when trying to advance beyond the end of the slice.
    #[inline]
    #[track_caller]
    pub fn advance(&mut self, n: usize) {
        if n > self.len() {
            slice_mut_overrun();
        }
        let whole = mem::replace(self, Self::new(&mut []));
        let rest = whole.into_slice().get_mut(n..).unwrap_or_default();
        *self = Self::new(rest);
    }

    /// Advances a slice of slices by `n` bytes, removing the buffers, empty
    /// ones included, that are fully advanced over.
    ///
    /// # Panics
    ///
    /// Panics when trying to advance beyond the end of the slices.
    #[inline]
    #[track_caller]
    #[allow(clippy::mut_mut, reason = "std's signature")]
    pub fn advance_slices(bufs: &mut &mut [Self], n: usize) {
        let (remove, left) = consumed(bufs.iter().map(|b| b.len()), n);
        *bufs = mem::take(bufs).get_mut(remove..).unwrap_or_default();
        match bufs.first_mut() {
            Some(first) => first.advance(left),
            None if left == 0 => {}
            None => slices_overrun(),
        }
    }
}

/// Returns how many of the leading buffers with the given lengths `n` bytes
/// cover completely, and the bytes left after them. Counting down avoids
/// summing lengths, which may exceed `usize::MAX` since buffers may alias.
#[inline]
fn consumed(lens: impl Iterator<Item = usize>, n: usize) -> (usize, usize) {
    let mut remove = 0;
    let mut left = n;
    for len in lens {
        match left.checked_sub(len) {
            Some(rest) => left = rest,
            None => break,
        }
        remove += 1;
    }
    (remove, left)
}

// `IoSlice::advance`, `IoSliceMut::advance` and `advance_slices` document
// that they panic when advancing past the end, with these messages.

#[cold]
#[inline(never)]
#[track_caller]
#[allow(clippy::panic, reason = "std documents this panic")]
fn slice_overrun() -> ! {
    panic!("advancing IoSlice beyond its length")
}

#[cold]
#[inline(never)]
#[track_caller]
#[allow(clippy::panic, reason = "std documents this panic")]
fn slice_mut_overrun() -> ! {
    panic!("advancing IoSliceMut beyond its length")
}

#[cold]
#[inline(never)]
#[track_caller]
#[allow(clippy::panic, reason = "std documents this panic")]
fn slices_overrun() -> ! {
    panic!("advancing io slices beyond their length")
}

impl Deref for IoSlice<'_> {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl Deref for IoSliceMut<'_> {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl DerefMut for IoSliceMut<'_> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [u8] {
        self.as_mut_slice()
    }
}

impl fmt::Debug for IoSlice<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_slice(), f)
    }
}

impl fmt::Debug for IoSliceMut<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_slice(), f)
    }
}

/// `iovec`, which `readv` and `writev` accept directly.
#[cfg(any(unix, target_os = "wasi"))]
mod raw {
    /// The pointer and length of a buffer, as `iovec` holds them.
    #[derive(Clone, Copy)]
    #[repr(transparent)]
    pub(super) struct Raw(libc::iovec);

    impl Raw {
        #[inline]
        pub(super) const fn new(ptr: *mut u8, len: usize) -> Self {
            Self(libc::iovec {
                iov_base: ptr.cast(),
                iov_len: len,
            })
        }

        #[inline]
        pub(super) const fn ptr(self) -> *mut u8 {
            self.0.iov_base.cast()
        }

        #[inline]
        pub(super) const fn len(self) -> usize {
            self.0.iov_len
        }
    }
}

/// `WSABUF`, which Winsock calls accept directly.
#[cfg(windows)]
mod raw {
    /// The pointer and length of a buffer, as `WSABUF` holds them: a 32-bit
    /// length, then the pointer.
    #[derive(Clone, Copy)]
    #[repr(C)]
    pub(super) struct Raw {
        len: u32,
        buf: *mut u8,
    }

    impl Raw {
        /// Describes `len` bytes at `ptr`, which must not be more than
        /// `u32::MAX`.
        #[inline]
        #[track_caller]
        pub(super) fn new(ptr: *mut u8, len: usize) -> Self {
            let Ok(len) = u32::try_from(len) else {
                too_long();
            };
            Self { len, buf: ptr }
        }

        #[inline]
        pub(super) const fn ptr(self) -> *mut u8 {
            self.buf
        }

        /// The length, which came from a `usize`, so the cast is lossless.
        #[inline]
        pub(super) const fn len(self) -> usize {
            self.len as usize
        }
    }

    // `IoSlice::new` and `IoSliceMut::new` document this panic on Windows.
    #[cold]
    #[inline(never)]
    #[track_caller]
    #[allow(clippy::panic, reason = "std documents this panic")]
    fn too_long() -> ! {
        panic!("assertion failed: buf.len() <= u32::MAX as usize")
    }
}

/// A pointer and a length, where no OS call takes a list of buffers.
#[cfg(not(any(unix, windows, target_os = "wasi")))]
mod raw {
    #[derive(Clone, Copy)]
    pub(super) struct Raw {
        ptr: *mut u8,
        len: usize,
    }

    impl Raw {
        #[inline]
        pub(super) const fn new(ptr: *mut u8, len: usize) -> Self {
            Self { ptr, len }
        }

        #[inline]
        pub(super) const fn ptr(self) -> *mut u8 {
            self.ptr
        }

        #[inline]
        pub(super) const fn len(self) -> usize {
            self.len
        }
    }
}
