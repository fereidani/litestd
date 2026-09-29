//! Pipes to a child process: anonymous pipes whose end in this process is
//! overlapped, so that `read_output` can read two at once, and whose end in
//! the child is synchronous, as children expect.

use core::{cell::UnsafeCell, ffi::c_void, mem::MaybeUninit, ptr};

use alloc_crate::{boxed::Box, string::String, vec::Vec};
use windows_sys::Win32::{
    Foundation::{
        ERROR_BROKEN_PIPE, ERROR_HANDLE_EOF, ERROR_IO_PENDING, NTSTATUS,
        STATUS_PENDING, WAIT_OBJECT_0,
    },
    Storage::FileSystem::{ReadFile, ReadFileEx, WriteFileEx},
    System::{
        IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
        Threading::{
            CreateEventW, CreateThread, INFINITE,
            STACK_SIZE_PARAM_IS_A_RESERVATION, SleepEx, WaitForMultipleObjects,
        },
    },
};

use super::super::{
    os::{
        self, cvt,
        handle::{Transfer, io_len, read_to_end_io, stream_io},
        last_error, win_error,
    },
    pipe::anon_pipe,
};
use crate::{
    io::{self, IoSlice, IoSliceMut},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
};

/// The stack reserved for a relay thread, whose frames are small.
const RELAY_STACK: usize = 64 * 1024;

/// Creates a pipe for a child's standard stream and returns this process's
/// end, overlapped and readable if `ours_readable` or else writable, and the
/// child's end, synchronous and inheritable. The caller holds the spawn
/// lock, so that only the child it starts inherits that end.
pub(super) fn pipe_pair(
    ours_readable: bool,
) -> io::Result<(ChildPipe, OwnedHandle)> {
    let (ours, theirs) = anon_pipe(ours_readable, true, true)?;
    Ok((ChildPipe { handle: ours }, theirs))
}

/// This process's end of a pipe to a child, opened for overlapped I/O.
pub(crate) struct ChildPipe {
    handle: OwnedHandle,
}

/// The error code and byte count that a transfer completed with.
type Completion = Option<(u32, u32)>;

/// The completion routine of [`ChildPipe::transfer`].
unsafe extern "system" fn completed(
    error: u32,
    transferred: u32,
    overlapped: *mut OVERLAPPED,
) {
    // SAFETY: `transfer` stores a pointer to its `Completion` in `hEvent`,
    // which `ReadFileEx` and `WriteFileEx` leave to the caller, and keeps
    // both alive until this routine has run.
    unsafe {
        (*overlapped)
            .hEvent
            .cast::<Completion>()
            .write(Some((error, transferred)));
    }
}

impl ChildPipe {
    /// Wraps a handle, which must be open for overlapped I/O.
    pub(crate) const fn from_handle(handle: OwnedHandle) -> Self {
        Self { handle }
    }

    pub(crate) const fn handle(&self) -> &OwnedHandle {
        &self.handle
    }

    pub(crate) fn into_handle(self) -> OwnedHandle {
        self.handle
    }

    /// Reads into `buf`, which only the kernel writes.
    pub(crate) fn read_uninit(
        &self,
        buf: &mut [MaybeUninit<u8>],
    ) -> io::Result<usize> {
        match self.transfer(Transfer::Read(buf)) {
            // Reading a pipe whose writer closed it fails with
            // `ERROR_BROKEN_PIPE`, which marks the end of the data.
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(0),
            result => result,
        }
    }

    pub(crate) fn write(&self, buf: &[u8]) -> io::Result<usize> {
        self.transfer(Transfer::Write(buf))
    }

    stream_io!();
    read_to_end_io!();

    /// Reads or writes once, with alertable I/O, as std does: the completion
    /// routine runs on this thread while it sleeps alertably, so concurrent
    /// transfers on the pipe, through `&ChildStdin`, cannot take each
    /// other's completion.
    fn transfer(&self, transfer: Transfer<'_>) -> io::Result<usize> {
        let mut completion: Completion = None;
        let slot = &raw mut completion;
        let mut overlapped = OVERLAPPED {
            hEvent: slot.cast(),
            ..OVERLAPPED::default()
        };
        let handle = self.handle.as_raw_handle();
        // SAFETY: the handle is open, and the buffer (`len` bytes) and
        // `overlapped` stay valid until the completion routine has run,
        // which this function waits for unless the call fails, in which
        // case the routine is never queued.
        let (ok, len) = unsafe {
            match transfer {
                Transfer::Read(buf) => {
                    let len = io_len(buf.len());
                    let buf = buf.as_mut_ptr().cast();
                    let op = &raw mut overlapped;
                    (ReadFileEx(handle, buf, len, op, Some(completed)), len)
                }
                Transfer::Write(buf) => {
                    let len = io_len(buf.len());
                    let op = &raw mut overlapped;
                    let ok = WriteFileEx(
                        handle,
                        buf.as_ptr(),
                        len,
                        op,
                        Some(completed),
                    );
                    (ok, len)
                }
            }
        };
        cvt(ok)?;
        // Ends once the routine has run: the transfer is queued, and
        // completes with data or with an error once the other end closes.
        let (error, transferred) = loop {
            // SAFETY: no preconditions; the wait is alertable, so queued
            // completion routines run in it.
            unsafe { SleepEx(INFINITE, 1) };
            // SAFETY: `slot` points to `completion`, which only the routine
            // writes, on this thread, inside `SleepEx`.
            if let Some(result) = unsafe { slot.read() } {
                break result;
            }
        };
        if error != 0 {
            return Err(win_error(error));
        }
        // A driver that reports more than the buffer holds would have the
        // caller trust bytes that were never written.
        Ok(transferred.min(len) as usize)
    }
}

/// Reads `p1` into `v1` and `p2` into `v2` until both end, with overlapped
/// reads on the two at once, so that a child blocked writing to one pipe
/// cannot deadlock with this process reading the other.
pub(crate) fn read_output(
    p1: ChildPipe,
    v1: &mut Vec<u8>,
    p2: ChildPipe,
    v2: &mut Vec<u8>,
) -> io::Result<()> {
    // The kernel writes the result of each read to its `OVERLAPPED`, which
    // outlives the read: both readers settle before this frame ends.
    let overlapped1 = UnsafeCell::new(OVERLAPPED::default());
    let overlapped2 = UnsafeCell::new(OVERLAPPED::default());
    let mut first = OverlappedRead::new(p1, v1, &overlapped1)?;
    let mut second = OverlappedRead::new(p2, v2, &overlapped2)?;
    let result = read_both(&mut first, &mut second);
    // After an error, a read may still be in flight.
    first.settle();
    second.settle();
    result
}

/// The loop of [`read_output`].
fn read_both<'a>(
    first: &mut OverlappedRead<'a>,
    second: &mut OverlappedRead<'a>,
) -> io::Result<()> {
    let events = [first.event.as_raw_handle(), second.event.as_raw_handle()];
    // Each pass completes a read and starts the next one on its pipe. Both
    // pipes end, when the child exits if not before, so one of them reaches
    // its end and the loop returns, unless a read fails first.
    loop {
        // SAFETY: both events are open.
        let signaled =
            unsafe { WaitForMultipleObjects(2, events.as_ptr(), 0, INFINITE) };
        let (this, other) = match signaled {
            WAIT_OBJECT_0 => (&mut *first, &mut *second),
            n if n == WAIT_OBJECT_0 + 1 => (&mut *second, &mut *first),
            _ => return Err(io::Error::last_os_error()),
        };
        if !(this.finish()? && this.start()?) {
            // Reads the other pipe to its end, one read at a time; it ends
            // as above.
            while other.finish()? && other.start()? {}
            return Ok(());
        }
    }
}

/// A pipe that [`read_output`] reads, with at most one overlapped read in
/// flight. While one is, the kernel writes to `overlapped` and the spare
/// capacity of `buf`, which only raw pointers reach until [`Self::finish`]
/// or [`Self::settle`] has waited for the read.
struct OverlappedRead<'a> {
    pipe: OwnedHandle,
    /// A manual-reset event that each read signals when it completes. It is
    /// created signaled, so that the first wait lets the first read start.
    event: OwnedHandle,
    overlapped: &'a UnsafeCell<OVERLAPPED>,
    buf: &'a mut Vec<u8>,
    /// The length of the read in flight, if there is one.
    pending: Option<usize>,
}

impl<'a> OverlappedRead<'a> {
    fn new(
        pipe: ChildPipe,
        buf: &'a mut Vec<u8>,
        overlapped: &'a UnsafeCell<OVERLAPPED>,
    ) -> io::Result<Self> {
        // SAFETY: an unnamed, signaled manual-reset event, not inheritable.
        let event = unsafe { CreateEventW(ptr::null(), 1, 1, ptr::null()) };
        if event.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `CreateEventW` returned a new handle that nothing owns.
        let event = unsafe { OwnedHandle::from_raw_handle(event) };
        // SAFETY: no read is in flight yet, so nothing else accesses it.
        unsafe { (*overlapped.get()).hEvent = event.as_raw_handle() };
        Ok(Self {
            pipe: pipe.handle,
            event,
            overlapped,
            buf,
            pending: None,
        })
    }

    /// Starts a read into the spare capacity of the buffer, which grows as
    /// in std. Returns `false` at the end of the data.
    fn start(&mut self) -> io::Result<bool> {
        debug_assert!(self.pending.is_none(), "one read at a time");
        if self.buf.len() == self.buf.capacity() {
            self.buf
                .reserve(if self.buf.capacity() == 0 { 16 } else { 1 });
        }
        let spare = self.buf.spare_capacity_mut();
        let len = io_len(spare.len());
        // SAFETY: the pipe is open for overlapped reads, and `spare` (`len`
        // bytes) and `overlapped` stay valid until the read has completed:
        // `pending` makes `finish` or `settle` wait for it.
        let ok = unsafe {
            ReadFile(
                self.pipe.as_raw_handle(),
                spare.as_mut_ptr().cast(),
                len,
                ptr::null_mut(),
                self.overlapped.get(),
            )
        };
        if ok == 0 {
            match last_error() {
                ERROR_IO_PENDING => {}
                ERROR_BROKEN_PIPE => return Ok(false),
                code => return Err(win_error(code)),
            }
        }
        // Whether it completed already or not, `finish` collects the result.
        self.pending = Some(len as usize);
        Ok(true)
    }

    /// Waits for the read in flight, if any, and keeps the bytes it read.
    /// Returns `false` at the end of the data.
    fn finish(&mut self) -> io::Result<bool> {
        let Some(len) = self.pending else {
            return Ok(true);
        };
        let mut read = 0;
        // SAFETY: `overlapped` belongs to the read in flight on the pipe, and
        // waiting makes the call return once that read has completed.
        let ok = unsafe {
            GetOverlappedResult(
                self.pipe.as_raw_handle(),
                self.overlapped.get(),
                &raw mut read,
                1,
            )
        };
        if ok == 0 {
            let code = last_error();
            if self.in_flight() {
                // The wait failed; `settle` deals with the read.
                return Err(win_error(code));
            }
            self.pending = None;
            return match code {
                ERROR_BROKEN_PIPE | ERROR_HANDLE_EOF => Ok(false),
                code => Err(win_error(code)),
            };
        }
        self.pending = None;
        // A driver that reports more than the buffer holds would have the
        // caller trust bytes that were never written. The buffer has not
        // changed since `start`, so the spare capacity is still `len`.
        let spare = self.buf.capacity() - self.buf.len();
        let read = (read as usize).min(len).min(spare);
        // SAFETY: the read initialized `read` bytes of the spare capacity,
        // which starts at `len()`, and `read` fits in it.
        unsafe { self.buf.set_len(self.buf.len() + read) };
        Ok(read != 0)
    }

    /// Whether the kernel still has the read in flight.
    fn in_flight(&self) -> bool {
        // SAFETY: `Internal` is initialized, and the kernel stores the status
        // there, possibly while this reads it, hence the volatile read. The
        // cast restores the `NTSTATUS` bits.
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let status = unsafe {
            ptr::read_volatile(&raw const (*self.overlapped.get()).Internal)
        } as NTSTATUS;
        status == STATUS_PENDING
    }

    /// Cancels the read in flight, if any, and waits until the kernel is
    /// done with the buffer. Aborts if the read stays in flight, as
    /// returning would free memory that the kernel is about to write.
    fn settle(&mut self) {
        if self.pending.is_none() {
            return;
        }
        let pipe = self.pipe.as_raw_handle();
        let mut read = 0;
        // SAFETY: `overlapped` belongs to the read in flight on the open
        // pipe; the wait returns once it has completed, cancelled or not.
        unsafe {
            CancelIoEx(pipe, self.overlapped.get());
            GetOverlappedResult(pipe, self.overlapped.get(), &raw mut read, 1);
        }
        if self.in_flight() {
            os::abort();
        }
        self.pending = None;
    }
}

/// The two ends that a relay thread copies between.
struct Relay {
    reader: ChildPipe,
    writer: ChildPipe,
}

/// Connects a child's standard stream to `source`, a pipe to another child,
/// through a new pipe and a thread that copies between the two, as std
/// does: `source` is overlapped, which the child could not use directly.
/// Returns the child's end, as for [`pipe_pair`].
pub(super) fn spawn_relay(
    source: &ChildPipe,
    ours_readable: bool,
) -> io::Result<OwnedHandle> {
    let source = ChildPipe {
        handle: source.handle.try_clone()?,
    };
    let (ours, theirs) = pipe_pair(ours_readable)?;
    let (reader, writer) = if ours_readable {
        (ours, source)
    } else {
        (source, ours)
    };
    let relay = Box::into_raw(Box::new(Relay { reader, writer }));
    // SAFETY: `relay_main` has the signature `CreateThread` expects and takes
    // over `relay`, a leaked box, as its parameter.
    let thread = unsafe {
        CreateThread(
            ptr::null(),
            RELAY_STACK,
            Some(relay_main),
            relay.cast_const().cast(),
            STACK_SIZE_PARAM_IS_A_RESERVATION,
            ptr::null_mut(),
        )
    };
    if thread.is_null() {
        let error = io::Error::last_os_error();
        // SAFETY: no thread started, so the box is still this function's.
        drop(unsafe { Box::from_raw(relay) });
        return Err(error);
    }
    // The relay runs detached until either pipe ends.
    // SAFETY: `CreateThread` returned a new handle that nothing else owns.
    drop(unsafe { OwnedHandle::from_raw_handle(thread) });
    Ok(theirs)
}

/// The body of a relay thread.
unsafe extern "system" fn relay_main(relay: *mut c_void) -> u32 {
    // SAFETY: `spawn_relay` passes a leaked `Box<Relay>` to this thread.
    let relay = unsafe { Box::from_raw(relay.cast::<Relay>()) };
    let mut buf = [0; 4096];
    // Ends at the end of the reader's data, or when either pipe fails, as
    // when its other end closes.
    while let Ok(len @ 1..) = relay.reader.read(&mut buf) {
        let mut data = buf.get(..len).unwrap_or_default();
        // Each pass writes at least one byte or ends the relay.
        while !data.is_empty() {
            match relay.writer.write(data) {
                Ok(n @ 1..) => data = data.get(n..).unwrap_or_default(),
                _ => return 0,
            }
        }
    }
    0
}
