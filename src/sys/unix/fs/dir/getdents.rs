//! Directory entries from `getdents64`, which safe code parses from the
//! `linux_dirent64` records with bounds checks. Opening a directory costs
//! one `openat`, without the `fstat` of `opendir`.

use core::{mem::MaybeUninit, slice};

use alloc_crate::boxed::Box;

use super::{valid_name, without_nul};
use crate::{
    io,
    os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
    sys::os::cvt_r,
};

/// An open directory: its descriptor.
pub(super) type Handle = OwnedFd;

/// Opens the directory stream of `fd`, a descriptor of the directory.
#[allow(clippy::unnecessary_wraps, reason = "macOS's `fdopendir` may fail")]
pub(super) const fn open(fd: OwnedFd) -> io::Result<Handle> {
    Ok(fd)
}

/// Bytes of records requested per `getdents64` call: a few hundred entries.
const BUF_SIZE: usize = 8 * 1024;

/// Offsets of the `linux_dirent64` fields read; the inode number is at 0.
const RECLEN: usize = 16;
const TYPE: usize = 18;
const NAME: usize = 19;

/// The position of one record in a `Records` buffer, and the fields read
/// from it.
#[derive(Clone, Copy, Default)]
struct Record {
    ino: u64,
    kind: u8,
    /// Start of the name in the buffer, and its length without the NUL.
    name_start: usize,
    name_len: usize,
}

/// Records read from a directory, and a cursor over them.
pub(super) struct Records {
    buf: Box<[MaybeUninit<u8>]>,
    /// Bytes the last `getdents64` call wrote; never more than `buf.len()`.
    len: usize,
    /// Offset of the next record.
    pos: usize,
    /// The entry `advance` moved to last.
    cur: Record,
}

impl Records {
    pub(super) fn new() -> Self {
        Self::with_capacity(BUF_SIZE)
    }

    /// Records without a buffer, which allocate nothing.
    pub(super) fn empty() -> Self {
        Self::with_capacity(0)
    }

    fn with_capacity(capacity: usize) -> Self {
        Self {
            buf: Box::new_uninit_slice(capacity),
            len: 0,
            pos: 0,
            cur: Record::default(),
        }
    }

    /// The bytes the kernel wrote.
    fn filled(&self) -> &[u8] {
        // `len` never exceeds the buffer, so this is never empty early.
        let filled = self.buf.get(..self.len).unwrap_or_default();
        // SAFETY: `filled` is the first `len` bytes of `buf`, which the last
        // `getdents64` call initialized; the borrow of `self` keeps them
        // unchanged. `MaybeUninit<u8>` has the layout of `u8`.
        unsafe {
            slice::from_raw_parts(filled.as_ptr().cast::<u8>(), filled.len())
        }
    }

    /// Moves to the next entry of `dir` other than `.` and `..`, refilling
    /// the buffer as needed. Returns `false` at the end of the directory.
    ///
    /// # Safety
    ///
    /// No other `Records` may read `dir` at the same time. Nothing unsound
    /// comes of it here, but macOS's streams require it, and `dir.rs`
    /// upholds it for both.
    pub(super) unsafe fn advance(&mut self, dir: &Handle) -> io::Result<bool> {
        // Each pass consumes a record of at least `NAME + 1` bytes or refills
        // the buffer; the kernel ends the directory by writing no bytes.
        loop {
            if self.pos >= self.len {
                self.len = getdents(dir.as_fd(), &mut self.buf)?;
                self.pos = 0;
                if self.len == 0 {
                    return Ok(false);
                }
            }
            let Some((record, next)) = parse(self.filled(), self.pos) else {
                // The kernel never writes a malformed record; stop reading
                // rather than trust the rest of the buffer.
                self.len = 0;
                return Err(super::MALFORMED);
            };
            self.cur = record;
            self.pos = next;
            let name = without_nul(self.name_with_nul());
            if name != b"." && name != b".." {
                return Ok(true);
            }
        }
    }

    /// The name of the current entry, NUL included.
    pub(super) fn name_with_nul(&self) -> &[u8] {
        let Record {
            name_start,
            name_len,
            ..
        } = self.cur;
        // `parse` found the name and its NUL in the buffer, so this never
        // falls back to an empty name.
        self.filled()
            .get(name_start..=name_start + name_len)
            .unwrap_or_default()
    }

    pub(super) const fn ino(&self) -> u64 {
        self.cur.ino
    }

    /// The `d_type` of the current entry.
    pub(super) const fn kind(&self) -> u8 {
        self.cur.kind
    }
}

/// Parses the record at `pos`, returning it and the offset of the next one,
/// or `None` if it does not fit in `bytes` or its name is not a single path
/// component.
fn parse(bytes: &[u8], pos: usize) -> Option<(Record, usize)> {
    let record = bytes.get(pos..)?;
    let reclen =
        u16::from_ne_bytes([*record.get(RECLEN)?, *record.get(RECLEN + 1)?]);
    let record = record.get(..usize::from(reclen))?;
    let ino = u64::from_ne_bytes(record.get(..8)?.try_into().ok()?);
    let kind = *record.get(TYPE)?;
    let name_len = record.get(NAME..)?.iter().position(|&b| b == 0)?;
    if !valid_name(record.get(NAME..NAME + name_len)?) {
        return None;
    }
    let record = Record {
        ino,
        kind,
        name_start: pos + NAME,
        name_len,
    };
    Some((record, pos + usize::from(reclen)))
}

/// Reads directory records from `fd` into `buf`, returning how many bytes
/// the kernel wrote: zero at the end of the directory.
fn getdents(
    fd: BorrowedFd<'_>,
    buf: &mut [MaybeUninit<u8>],
) -> io::Result<usize> {
    let (ptr, len) = (buf.as_mut_ptr(), buf.len());
    // SAFETY: `buf` is valid for writes of `len` bytes, and `getdents64`
    // writes whole records within that length.
    let n = cvt_r(|| unsafe {
        libc::syscall(libc::SYS_getdents64, fd.as_raw_fd(), ptr, len)
    })?;
    // The kernel never reports more than it was given room for.
    Ok(usize::try_from(n).map_or(0, |n| n.min(len)))
}
