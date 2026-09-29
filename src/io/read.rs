//! The [`Read`] trait and the algorithms behind its provided methods.
//!
//! The provided methods are thin generic loops around `read`; buffer growth,
//! initialization tracking and UTF-8 validation live in non-generic code that
//! all readers share.
//!
//! [`Read::read`] takes initialized memory, since a reader may read the buffer
//! it is lent, so the provided [`Read::read_to_end`] zeroes the spare capacity
//! of the vector first, each byte at most once per call. [`read_to_end_uninit`]
//! lends it uninitialized instead, for readers whose data only the OS writes:
//! files, sockets and pipes.

use core::{mem::MaybeUninit, slice, str};

use alloc_crate::{string::String, vec::Vec};

use super::{Bytes, Chain, Error, IoSliceMut, Result, Take};

/// The buffer size of `BufReader` and `BufWriter`, and the size of the stack
/// buffer of `copy`.
pub(crate) const DEFAULT_BUF_SIZE: usize = 8 * 1024;

/// The `Read` trait allows for reading bytes from a source, through one
/// required method, [`read`](Read::read).
pub trait Read {
    /// Pulls some bytes from this source into the specified buffer, returning
    /// how many bytes were read.
    ///
    /// `Ok(n)` must satisfy `n <= buf.len()`, though callers cannot rely on it
    /// for safety; `Ok(0)` means end of file or an empty `buf`.
    /// Implementations should not read from `buf`.
    ///
    /// # Errors
    ///
    /// Any I/O error, after which no bytes were read. An
    /// [`Interrupted`](super::ErrorKind::Interrupted) error is non-fatal.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize>;

    /// Like [`read`](Read::read), except that it reads into a slice of
    /// buffers, filled in order as one `read` into their concatenation would.
    /// The default reads into the first nonempty buffer.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`read`](Read::read).
    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> Result<usize> {
        self.read(first_nonempty_mut(bufs))
    }

    /// Reads all bytes until EOF in this source, appending them to `buf`, and
    /// returns how many bytes were read.
    ///
    /// # Errors
    ///
    /// On an error other than [`Interrupted`](super::ErrorKind::Interrupted),
    /// which is retried, `buf` keeps the bytes read so far.
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
        default_read_to_end(self, buf)
    }

    /// Reads all bytes until EOF in this source, appending them to `buf`, and
    /// returns how many bytes were read.
    ///
    /// # Errors
    ///
    /// Fails without changing `buf` if the data is not valid UTF-8; after a
    /// read error, `buf` keeps the bytes read only if they are valid UTF-8.
    fn read_to_string(&mut self, buf: &mut String) -> Result<usize> {
        default_read_to_string(self, buf)
    }

    /// Reads the exact number of bytes required to fill `buf`.
    ///
    /// # Errors
    ///
    /// [`UnexpectedEof`](super::ErrorKind::UnexpectedEof) if the source ends
    /// early; after any error the contents of `buf` are unspecified.
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        default_read_exact(self, buf)
    }

    /// Creates a "by reference" adapter for this instance of `Read`.
    fn by_ref(&mut self) -> &mut Self
    where
        Self: Sized,
    {
        self
    }

    /// Transforms this `Read` instance to an [`Iterator`] over its bytes, one
    /// [`read`](Read::read) call per byte.
    fn bytes(self) -> Bytes<Self>
    where
        Self: Sized,
    {
        Bytes { inner: self }
    }

    /// Creates an adapter which will read from this reader until it reaches
    /// EOF, and then from `next`.
    fn chain<R: Read>(self, next: R) -> Chain<Self, R>
    where
        Self: Sized,
    {
        Chain::new(self, next)
    }

    /// Creates an adapter which will read at most `limit` bytes from this
    /// reader.
    fn take(self, limit: u64) -> Take<Self>
    where
        Self: Sized,
    {
        Take::new(self, limit)
    }
}

/// Reads all bytes from a reader into a new [`String`].
///
/// # Errors
///
/// Returns the errors of [`Read::read_to_string`].
pub fn read_to_string<R: Read>(mut reader: R) -> Result<String> {
    let mut buf = String::new();
    reader.read_to_string(&mut buf)?;
    Ok(buf)
}

/// Returns the first nonempty buffer of `bufs`, or an empty one.
fn first_nonempty_mut<'a>(bufs: &'a mut [IoSliceMut<'_>]) -> &'a mut [u8] {
    bufs.iter_mut()
        .find(|b| !b.is_empty())
        .map_or(&mut [][..], |b| &mut **b)
}

/// The provided [`Read::read_exact`].
pub(crate) fn default_read_exact<R: Read + ?Sized>(
    reader: &mut R,
    mut buf: &mut [u8],
) -> Result<()> {
    // Each pass fills at least one byte, stops at EOF or on an error, or
    // retries after an interruption, so the loop ends once `buf` is full.
    while !buf.is_empty() {
        match reader.read(buf) {
            Ok(0) => return Err(Error::READ_EXACT_EOF),
            // A count beyond the buffer, which breaks `read`'s contract,
            // fills it.
            Ok(n) => buf = buf.get_mut(n..).unwrap_or_default(),
            Err(e) if e.is_interrupted() => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// The provided [`Read::read_to_end`]. Reads start at [`DEFAULT_BUF_SIZE`]
/// and double whenever one is filled, up to [`MAX_ZEROED_READ`].
fn default_read_to_end<R: Read + ?Sized>(
    reader: &mut R,
    buf: &mut Vec<u8>,
) -> Result<usize> {
    let mut state = ReadToEnd::new(buf, None, true);
    // Ends at EOF or on an error other than an interruption.
    loop {
        let result = reader.read(state.next_initialized()?);
        // SAFETY: the buffer was initialized when it was lent out, and safe
        // code cannot de-initialize it.
        if let Some(total) = unsafe { state.finish(result) }? {
            return Ok(total);
        }
    }
}

/// The provided [`Read::read_to_string`]: [`default_read_to_end`] with
/// UTF-8 validation.
fn default_read_to_string<R: Read + ?Sized>(
    reader: &mut R,
    buf: &mut String,
) -> Result<usize> {
    // SAFETY: `default_read_to_end` only appends to the vector.
    unsafe { append_to_string(buf, |bytes| default_read_to_end(reader, bytes)) }
}

/// Views an initialized buffer as one to read into.
///
/// # Safety
///
/// Nothing may write uninitialized bytes through the result, which holds for
/// readers that lend it to the OS alone.
// The readers that lend the OS uninitialized buffers; wasm32-unknown-unknown
// has none but the portable `fs` code.
#[cfg(any(not(target_os = "unknown"), feature = "fs"))]
pub(crate) const unsafe fn as_uninit(buf: &mut [u8]) -> &mut [MaybeUninit<u8>] {
    // SAFETY: `MaybeUninit<u8>` has the layout of `u8`, and the caller keeps
    // the bytes initialized.
    unsafe { &mut *(core::ptr::from_mut(buf) as *mut [MaybeUninit<u8>]) }
}

/// Like the provided [`Read::read_to_end`], but lends `read` the spare
/// capacity of `buf` uninitialized, for readers whose data only the OS writes.
///
/// `size_hint` estimates the bytes left. It is reserved up front, failing with
/// an `OutOfMemory` error as in std if that is impossible, and caps each read
/// at the hint plus some slack; without it, reads are unbounded.
///
/// # Safety
///
/// `read` must not read its buffer or write uninitialized bytes to it, and on
/// `Ok(n)` must have initialized its first `n` bytes, or all if `n` is larger.
#[cfg(any(not(target_os = "unknown"), feature = "fs"))]
pub(crate) unsafe fn read_to_end_uninit<F>(
    buf: &mut Vec<u8>,
    size_hint: Option<usize>,
    mut read: F,
) -> Result<usize>
where
    F: FnMut(&mut [MaybeUninit<u8>]) -> Result<usize>,
{
    buf.try_reserve(size_hint.unwrap_or(0))?;
    let mut state = UninitReadToEnd(ReadToEnd::new(buf, size_hint, false));
    // Ends at EOF or on an error other than an interruption.
    loop {
        // SAFETY: the caller upholds the contract for `read`.
        if let Some(total) = unsafe { state.read_once(&mut read) }? {
            return Ok(total);
        }
    }
}

/// A [`read_to_end_uninit`] without size hint that the caller drives one
/// read at a time, as when `poll` tells which of two pipes to read.
#[cfg(any(not(target_os = "unknown"), feature = "fs"))]
pub(crate) struct UninitReadToEnd<'a>(ReadToEnd<'a>);

#[cfg(any(not(target_os = "unknown"), feature = "fs"))]
impl<'a> UninitReadToEnd<'a> {
    #[cfg_attr(
        not(all(unix, feature = "command")),
        allow(dead_code, reason = "for the pipes of a child on Unix")
    )]
    pub(crate) fn new(buf: &'a mut Vec<u8>) -> Self {
        Self(ReadToEnd::new(buf, None, false))
    }

    /// Reads once into the vector, returning the total read at EOF, or
    /// `None` to read again, as after an interruption.
    ///
    /// # Safety
    ///
    /// As for [`read_to_end_uninit`].
    pub(crate) unsafe fn read_once<F>(
        &mut self,
        read: F,
    ) -> Result<Option<usize>>
    where
        F: FnOnce(&mut [MaybeUninit<u8>]) -> Result<usize>,
    {
        let result = read(self.0.next_uninit()?);
        // SAFETY: the caller guarantees that `read` initialized what it
        // reports.
        unsafe { self.0.finish(result) }
    }
}

/// [`read_to_end_uninit`] with the UTF-8 validation of
/// [`Read::read_to_string`].
///
/// # Safety
///
/// As for [`read_to_end_uninit`].
#[cfg(feature = "fs")]
pub(crate) unsafe fn read_to_string_uninit<F>(
    buf: &mut String,
    size_hint: Option<usize>,
    read: F,
) -> Result<usize>
where
    F: FnMut(&mut [MaybeUninit<u8>]) -> Result<usize>,
{
    // SAFETY: `read_to_end_uninit` only appends to the vector, and the
    // caller upholds its contract.
    unsafe {
        append_to_string(buf, |bytes| {
            read_to_end_uninit(bytes, size_hint, read)
        })
    }
}

/// The size of the stack buffer that `read_to_end` reads into while the
/// vector might be an exact fit, so that reaching EOF does not grow it. At
/// least large enough for any UTF-8 encoded character.
const PROBE_SIZE: usize = 32;

/// The largest read into memory that has to be zeroed first. Reads this large
/// already cost little per call, and zeroing further ahead of the reader would
/// write each byte to memory twice instead of in the cache.
const MAX_ZEROED_READ: usize = 256 * 1024;
const _: () = assert!(DEFAULT_BUF_SIZE <= MAX_ZEROED_READ);

/// The state of one `read_to_end` call: how the vector grows, how much of
/// its spare capacity is initialized, and how large the next read is.
struct ReadToEnd<'a> {
    buf: &'a mut Vec<u8>,
    start_len: usize,
    start_cap: usize,
    /// The number of initialized bytes at the start of the spare capacity,
    /// which is never more than the spare capacity.
    init: usize,
    /// The largest read to ask for.
    max_read: usize,
    /// Whether `max_read` grows with the reader: for memory zeroed first,
    /// without a size hint.
    adaptive: bool,
    /// The length of the buffer lent out last, or `None` for `probe`.
    lent: Option<usize>,
    probe: [u8; PROBE_SIZE],
}

impl<'a> ReadToEnd<'a> {
    /// Starts reading into `buf`. Reads are capped at the size hint plus some
    /// slack; without a hint, memory that is `zeroed` first is read into
    /// `DEFAULT_BUF_SIZE` at a time, doubling as the reader fills it, and
    /// memory that the OS fills is read into whole, where std caps the first
    /// read of any reader.
    fn new(
        buf: &'a mut Vec<u8>,
        size_hint: Option<usize>,
        zeroed: bool,
    ) -> Self {
        let unhinted = if zeroed { DEFAULT_BUF_SIZE } else { usize::MAX };
        let max_read = size_hint
            .and_then(|s| {
                s.checked_add(1024)?
                    .checked_next_multiple_of(DEFAULT_BUF_SIZE)
            })
            .unwrap_or(unhinted);
        Self {
            start_len: buf.len(),
            start_cap: buf.capacity(),
            buf,
            init: 0,
            max_read,
            adaptive: zeroed && size_hint.is_none(),
            lent: None,
            probe: [0; PROBE_SIZE],
        }
    }

    /// Picks the next buffer to read into, growing the vector if it is full:
    /// the spare capacity, capped at `max_read` or at what is initialized, or
    /// `probe` while the vector might be an exact fit. Returns the length to
    /// lend, at most the spare capacity, or `None` for `probe`.
    fn plan(&mut self) -> Result<Option<usize>> {
        let mut spare = self.buf.capacity() - self.buf.len();
        if spare < PROBE_SIZE {
            if self.buf.capacity() == self.start_cap {
                self.lent = None;
                return Ok(None);
            }
            self.buf.try_reserve(PROBE_SIZE)?;
            // The contents of the spare capacity do not survive growth.
            self.init = 0;
            spare = self.buf.capacity() - self.buf.len();
        }
        let len = if self.init > PROBE_SIZE {
            self.init
        } else {
            self.max_read
        }
        .min(spare);
        self.lent = Some(len);
        Ok(Some(len))
    }

    /// Lends out the next buffer to read into, initialized.
    fn next_initialized(&mut self) -> Result<&mut [u8]> {
        let Some(len) = self.plan()? else {
            return Ok(&mut self.probe);
        };
        let spare = self.buf.spare_capacity_mut();
        let spare = spare.get_mut(..len).unwrap_or_default();
        if let Some(fresh) = spare.get_mut(self.init..) {
            fresh.fill(MaybeUninit::new(0));
        }
        self.init = self.init.max(spare.len());
        // SAFETY: `spare` is initialized: its first `init` bytes, as counted
        // before the `fill`, were zeroed or read into by earlier calls and
        // cannot have been de-initialized through the `&mut [u8]` they were
        // lent as, and the `fill` initialized the rest. `MaybeUninit<u8>` has
        // the size and alignment of `u8`.
        Ok(unsafe {
            slice::from_raw_parts_mut(spare.as_mut_ptr().cast(), spare.len())
        })
    }

    /// Lends out the next buffer to read into, possibly uninitialized.
    #[cfg(any(not(target_os = "unknown"), feature = "fs"))]
    fn next_uninit(&mut self) -> Result<&mut [MaybeUninit<u8>]> {
        let Some(len) = self.plan()? else {
            // SAFETY: the readers that `read_to_end_uninit` accepts never
            // write uninitialized bytes, so `probe` stays initialized.
            return Ok(unsafe { as_uninit(&mut self.probe) });
        };
        let spare = self.buf.spare_capacity_mut();
        Ok(spare.get_mut(..len).unwrap_or_default())
    }

    /// Accounts for the result of a read into the last buffer lent out,
    /// returning the total read at EOF, or `None` to read again.
    ///
    /// # Safety
    ///
    /// On `Ok(n)`, the first `n` bytes of that buffer, or all of it if `n` is
    /// larger, must be initialized.
    unsafe fn finish(
        &mut self,
        result: Result<usize>,
    ) -> Result<Option<usize>> {
        let n = match result {
            Ok(n) => n,
            Err(e) if e.is_interrupted() => return Ok(None),
            Err(e) => return Err(e),
        };
        let Some(lent) = self.lent else {
            let probed = self.probe.get(..n).unwrap_or(&self.probe);
            if probed.is_empty() {
                return Ok(Some(self.buf.len() - self.start_len));
            }
            // The data is read already, so a failed allocation cannot be
            // reported without losing it.
            self.buf.extend_from_slice(probed);
            self.init = 0;
            return Ok(None);
        };
        // `lent` fits in the spare capacity, untouched since it was lent;
        // clamping to it anyway keeps `set_len` in bounds on its own.
        let n = n.min(lent).min(self.buf.capacity() - self.buf.len());
        // SAFETY: `len + n <= capacity` by the clamp above, and the `n`
        // bytes after `len` begin the buffer lent last, whose first `n` bytes
        // the caller guarantees are initialized.
        unsafe { self.buf.set_len(self.buf.len() + n) };
        self.init = self.init.saturating_sub(n);
        if n == 0 {
            return Ok(Some(self.buf.len() - self.start_len));
        }
        // Adaptive reads start at `DEFAULT_BUF_SIZE`, below the cap.
        if self.adaptive && n == self.max_read {
            self.max_read = n.saturating_mul(2).min(MAX_ZEROED_READ);
        }
        Ok(None)
    }
}

/// Appends to `buf` with `f`, keeping what `f` appended only if it is valid
/// UTF-8; otherwise `buf` is unchanged and the result is `f`'s error, or an
/// `InvalidData` error if `f` succeeded.
///
/// # Safety
///
/// `f` must neither change nor remove the bytes the vector holds when it is
/// called, which is trivially true for an empty `buf`.
pub(crate) unsafe fn append_to_string<F>(
    buf: &mut String,
    f: F,
) -> Result<usize>
where
    F: FnOnce(&mut Vec<u8>) -> Result<usize>,
{
    // Owning the bytes while `f` runs leaves `buf` a valid string, empty,
    // should `f` unwind.
    let mut bytes = core::mem::take(buf).into_bytes();
    let old_len = bytes.len();
    let result = f(&mut bytes);
    // SAFETY: the caller guarantees that `f` kept the first `old_len` bytes.
    unsafe { finish_append(buf, bytes, old_len, result) }
}

/// The non-generic part of [`append_to_string`].
///
/// # Safety
///
/// `bytes[..old_len]` must be valid UTF-8 that ends on a character
/// boundary.
unsafe fn finish_append(
    buf: &mut String,
    mut bytes: Vec<u8>,
    old_len: usize,
    result: Result<usize>,
) -> Result<usize> {
    let appended = bytes.get(old_len..).unwrap_or_default();
    let result = if str::from_utf8(appended).is_ok() {
        result
    } else {
        bytes.truncate(old_len);
        result.and(Err(Error::INVALID_UTF8))
    };
    // SAFETY: the bytes up to `old_len` are valid UTF-8 ending on a
    // character boundary, as the caller guarantees, and what follows them
    // was validated above or truncated away.
    *buf = unsafe { String::from_utf8_unchecked(bytes) };
    result
}
