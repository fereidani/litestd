//! UTF-16 strings as the Windows API takes and returns them: NUL-terminated,
//! in stack buffers unless longer than [`STACK_UNITS`], and written by
//! functions that report the size they need.

use core::{mem::MaybeUninit, slice};

use alloc_crate::vec::Vec;
use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, SetLastError};

use super::{errno, win_error};
#[cfg(any(feature = "command", feature = "fs"))]
use crate::path::PathBuf;
use crate::{
    ffi::{OsStr, OsString},
    io,
    os::windows::ffi::{OsStrExt, OsStringExt},
};

/// UTF-16 units a [`WideBuf`] holds on the stack: paths under 384 bytes fit,
/// as on Unix, with room for the verbatim prefix and a search pattern.
const STACK_UNITS: usize = 392;

/// Attempts at a size-reporting call before giving up. The size needed
/// changes only if the result changes between two calls.
const FILL_ATTEMPTS: usize = 8;

/// std's error for strings with an interior NUL.
pub(crate) const NUL_ERROR: io::Error = io::const_error!(
    io::ErrorKind::InvalidInput,
    "strings passed to WinAPI cannot contain NULs",
);

/// A UTF-16 buffer on the stack, backed by the heap for longer contents.
pub(crate) struct WideBuf {
    stack: [MaybeUninit<u16>; STACK_UNITS],
    heap: Vec<u16>,
}

impl WideBuf {
    /// Creates an empty buffer. Nothing is initialized or allocated.
    pub(crate) const fn new() -> Self {
        Self {
            stack: [MaybeUninit::uninit(); STACK_UNITS],
            heap: Vec::new(),
        }
    }

    /// Returns space for at least `len` units, on the heap if needed.
    fn space(&mut self, len: usize) -> &mut [MaybeUninit<u16>] {
        if len <= STACK_UNITS {
            &mut self.stack
        } else {
            self.heap.clear();
            self.heap.reserve_exact(len);
            self.heap.spare_capacity_mut()
        }
    }

    /// Calls `f(buf, capacity)` with growing buffers until its string fits,
    /// and returns the space written with the string's length. The string
    /// starts `offset` units into the space and is NUL-terminated.
    ///
    /// `f` returns what size-reporting Windows functions do: the length
    /// without the NUL on success, the capacity needed (NUL included) if
    /// `capacity` is too small, or 0 with the last error set. A length below
    /// its call's capacity means `f` wrote that many units and a NUL. The
    /// space is zeroed before each call, so the units up to that length and
    /// the NUL are initialized even if Windows wrote fewer.
    #[inline(never)]
    pub(crate) fn fill(
        &mut self,
        offset: usize,
        f: &mut dyn FnMut(*mut u16, u32) -> u32,
    ) -> io::Result<(&mut [MaybeUninit<u16>], usize)> {
        let mut capacity = STACK_UNITS.saturating_sub(offset);
        let mut result = None;
        for _ in 0..FILL_ATTEMPTS {
            let needed = offset.saturating_add(capacity);
            let space = self.space(needed);
            let Some(tail) = space.get_mut(offset..) else {
                break;
            };
            tail.fill(MaybeUninit::new(0));
            // Windows strings are far shorter than `u32::MAX` units; a
            // smaller capacity is still correct.
            let size = u32::try_from(tail.len()).unwrap_or(u32::MAX);
            // SAFETY: `SetLastError` has no preconditions. Clearing the
            // error tells an empty result apart from a failure below.
            unsafe { SetLastError(0) };
            let len = f(tail.as_mut_ptr().cast(), size) as usize;
            if len == 0 && errno() != 0 {
                return Err(io::Error::last_os_error());
            }
            if len < tail.len() {
                result = Some((needed > STACK_UNITS, len));
                break;
            }
            // Too small: `len` is the capacity needed, unless the function
            // reported a size that does not help, which is then doubled.
            capacity = if len > capacity {
                len
            } else {
                capacity.saturating_mul(2)
            };
        }
        let Some((on_heap, len)) = result else {
            return Err(win_error(ERROR_INSUFFICIENT_BUFFER));
        };
        // The space of the successful call, untouched by `space` since.
        let space = if on_heap {
            self.heap.spare_capacity_mut()
        } else {
            &mut self.stack[..]
        };
        Ok((space, len))
    }

    /// Like [`Self::fill`] without an offset, returning the string that `f`
    /// wrote, without its NUL.
    pub(crate) fn fill_wide(
        &mut self,
        f: &mut dyn FnMut(*mut u16, u32) -> u32,
    ) -> io::Result<&[u16]> {
        let (space, len) = self.fill(0, f)?;
        let written = space.get(..len).unwrap_or_default();
        // SAFETY: `fill` returned the length of a successful call, below the
        // length of the space that it zeroed for that call.
        Ok(unsafe { assume_init(written) })
    }
}

/// Returns the string that `f` writes, called as [`WideBuf::fill`] calls
/// it.
pub(crate) fn fill_os_string(
    f: &mut dyn FnMut(*mut u16, u32) -> u32,
) -> io::Result<OsString> {
    WideBuf::new().fill_wide(f).map(OsString::from_wide)
}

/// Views initialized units as `u16`s.
///
/// # Safety
///
/// Every unit of `init` must be initialized.
pub(crate) const unsafe fn assume_init(init: &[MaybeUninit<u16>]) -> &[u16] {
    // SAFETY: `MaybeUninit<u16>` has the layout of `u16`, so `init` spans
    // `init.len()` valid `u16` slots, all initialized per the caller; the
    // result borrows `init`, so nothing writes to them while it lives.
    unsafe { slice::from_raw_parts(init.as_ptr().cast::<u16>(), init.len()) }
}

/// Converts `s` followed by `suffix` to NUL-terminated UTF-16 in `buf`, and
/// returns the units written, NUL included. A NUL in `s` fails with
/// [`NUL_ERROR`].
#[inline(never)]
pub(crate) fn to_wide<'a>(
    buf: &'a mut WideBuf,
    s: &OsStr,
    suffix: &[u16],
) -> io::Result<&'a [u16]> {
    // WTF-8 never needs more units than bytes.
    let space = buf.space(s.len() + suffix.len() + 1);
    // Only a NUL character of `s` encodes to a zero unit, and it is invalid.
    let units = s
        .encode_wide()
        .map(|unit| (unit, unit != 0))
        .chain(suffix.iter().chain(&[0]).map(|&unit| (unit, true)));
    let mut slots = space.iter_mut();
    let mut len = 0;
    for (unit, valid) in units {
        if !valid {
            return Err(NUL_ERROR);
        }
        let Some(slot) = slots.next() else {
            return Err(win_error(ERROR_INSUFFICIENT_BUFFER));
        };
        slot.write(unit);
        len += 1;
    }
    let init = space.get(..len).unwrap_or_default();
    // SAFETY: the loop wrote the first `len` units of `space`.
    Ok(unsafe { assume_init(init) })
}

/// Converts UTF-16 from the Windows API to a path.
#[cfg(any(feature = "command", feature = "fs"))]
pub(crate) fn to_path_buf(wide: &[u16]) -> PathBuf {
    PathBuf::from(OsString::from_wide(wide))
}
