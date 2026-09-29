//! The environment block of this process, as `GetEnvironmentStringsW` hands
//! it out.

use core::slice;

use alloc_crate::vec::Vec;
use windows_sys::Win32::System::Environment::{
    FreeEnvironmentStringsW, GetEnvironmentStringsW,
};

use crate::io;

/// The unit that separates a variable's name from its value.
pub(crate) const EQUALS: u16 = b'=' as u16;

/// A copy of the environment block, freed on drop: `NAME=VALUE` strings, each
/// followed by a NUL, and an empty string after the last.
pub(crate) struct EnvironmentStrings {
    block: *mut u16,
    /// The units before the empty string that ends the block.
    len: usize,
}

impl EnvironmentStrings {
    pub(crate) fn get() -> io::Result<Self> {
        // SAFETY: no preconditions.
        let block = unsafe { GetEnvironmentStringsW() };
        if block.is_null() {
            return Err(io::Error::last_os_error());
        }
        debug_assert!(block.is_aligned());
        let mut len = 0;
        // Each pass moves past one NUL-terminated string; the block ends
        // with an empty one, which ends the loop.
        // SAFETY: the block consists of NUL-terminated strings followed by
        // an empty one, so every unit up to that one can be read.
        while unsafe { *block.add(len) } != 0 {
            // SAFETY: as above; this string ends with a NUL.
            while unsafe { *block.add(len) } != 0 {
                len += 1;
            }
            len += 1;
        }
        Ok(Self { block, len })
    }

    /// The units before the empty string that ends the block.
    pub(crate) const fn units(&self) -> &[u16] {
        // SAFETY: the first `len` units of the block are initialized, as
        // `get` read them, and stay valid until `self` frees the block.
        unsafe { slice::from_raw_parts(self.block, self.len) }
    }

    /// The variables, as `(name, value)` pairs in the block's order. As in
    /// std, a name may start with `=`, and a string with no other `=` is
    /// skipped.
    pub(crate) fn vars(&self) -> impl Iterator<Item = (&[u16], &[u16])> {
        self.units().split(|&unit| unit == 0).filter_map(|var| {
            let eq = var.iter().skip(1).position(|&u| u == EQUALS)? + 1;
            Some((var.get(..eq)?, var.get(eq + 1..)?))
        })
    }
}

impl Drop for EnvironmentStrings {
    fn drop(&mut self) {
        // SAFETY: the block came from `GetEnvironmentStringsW` and is freed
        // once, here. Freeing cannot fail on it.
        unsafe { FreeEnvironmentStringsW(self.block) };
    }
}

/// Appends `NAME=VALUE` and a NUL.
pub(crate) fn push_var(block: &mut Vec<u16>, name: &[u16], value: &[u16]) {
    block.reserve(name.len() + value.len() + 2);
    block.extend_from_slice(name);
    block.push(EQUALS);
    block.extend_from_slice(value);
    block.push(0);
}
