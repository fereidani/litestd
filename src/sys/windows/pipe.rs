//! Anonymous pipes, created through the NT API as std creates the pipes to a
//! child, so that one function makes both kinds: [`Pipe`], the synchronous
//! ends of `io::pipe`, and the pipes to a child, whose end in this process
//! is overlapped.

use core::{
    ffi::c_void,
    fmt,
    mem::MaybeUninit,
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};

use alloc_crate::{string::String, vec::Vec};
use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            FILE_CREATE, FILE_NON_DIRECTORY_FILE, FILE_PIPE_BYTE_STREAM_MODE,
            FILE_PIPE_BYTE_STREAM_TYPE, FILE_PIPE_QUEUE_OPERATION,
            FILE_SYNCHRONOUS_IO_NONALERT, NtOpenFile,
        },
    },
    Win32::{
        Foundation::{
            CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, NTSTATUS,
            OBJ_INHERIT, UNICODE_STRING,
        },
        Storage::FileSystem::{
            FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
            FILE_WRITE_ATTRIBUTES, SYNCHRONIZE,
        },
        System::IO::IO_STATUS_BLOCK,
    },
    core::w,
};

use super::os::{
    handle::{self, read_to_end_io, stream_io},
    nt_error,
};
use crate::{
    io::{self, IoSlice, IoSliceMut},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
};

// windows-sys lacks it; std declares it the same way.
#[cfg_attr(
    target_arch = "x86",
    link(
        name = "ntdll.dll",
        kind = "raw-dylib",
        modifiers = "+verbatim",
        import_name_type = "undecorated"
    )
)]
#[cfg_attr(
    not(target_arch = "x86"),
    link(name = "ntdll.dll", kind = "raw-dylib", modifiers = "+verbatim")
)]
unsafe extern "system" {
    fn NtCreateNamedPipeFile(
        file_handle: *mut HANDLE,
        desired_access: u32,
        object_attributes: *const OBJECT_ATTRIBUTES,
        io_status_block: *mut IO_STATUS_BLOCK,
        share_access: u32,
        create_disposition: u32,
        create_options: u32,
        named_pipe_type: u32,
        read_mode: u32,
        completion_mode: u32,
        maximum_instances: u32,
        inbound_quota: u32,
        outbound_quota: u32,
        default_timeout: *const i64,
    ) -> NTSTATUS;
}

/// The buffer size of each direction, as on Linux.
const PIPE_CAPACITY: u32 = 64 * 1024;

/// `\Device\NamedPipe\`, the directory of the pipe file system, opened on
/// first use and kept open for the life of the process.
static PIPE_FS: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

/// Returns the handle in [`PIPE_FS`], opening it if needed.
fn pipe_fs() -> io::Result<HANDLE> {
    // A handle names a kernel object that exists before it is published,
    // and no memory is published with it, so `Relaxed` suffices.
    let cached = PIPE_FS.load(Ordering::Relaxed);
    if !cached.is_null() {
        return Ok(cached);
    }
    // The kernel only reads the name, 18 units.
    let name = UNICODE_STRING {
        Length: 36,
        MaximumLength: 36,
        Buffer: w!(r"\Device\NamedPipe\").cast_mut(),
    };
    let attributes = object_attributes(ptr::null_mut(), &name);
    let mut handle = ptr::null_mut();
    let mut status_block = IO_STATUS_BLOCK::default();
    // SAFETY: every pointer is valid for the call, which completes before
    // returning on this synchronous open.
    let status = unsafe {
        NtOpenFile(
            &raw mut handle,
            SYNCHRONIZE | GENERIC_READ,
            &raw const attributes,
            &raw mut status_block,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_SYNCHRONOUS_IO_NONALERT,
        )
    };
    if status < 0 {
        return Err(nt_error(status));
    }
    match PIPE_FS.compare_exchange(
        ptr::null_mut(),
        handle,
        Ordering::Relaxed,
        Ordering::Relaxed,
    ) {
        Ok(_) => Ok(handle),
        Err(existing) => {
            // SAFETY: another thread published its handle first, so this
            // one, which nothing else knows, is closed.
            unsafe { CloseHandle(handle) };
            Ok(existing)
        }
    }
}

/// Object attributes naming `name` relative to `root`, not inheritable.
fn object_attributes(root: HANDLE, name: &UNICODE_STRING) -> OBJECT_ATTRIBUTES {
    OBJECT_ATTRIBUTES {
        // The structure is a few dozen bytes long.
        #[allow(clippy::cast_possible_truncation)]
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: root,
        ObjectName: name,
        ..OBJECT_ATTRIBUTES::default()
    }
}

/// Creates an anonymous pipe and returns its server end, readable if
/// `server_reads` and writable otherwise, and its client end, for the other
/// direction. The server end is overlapped if `overlapped`; the client end
/// is synchronous, and inheritable if `inheritable`, which is for a child's
/// end while the spawn lock is held. Each end has the access rights of the
/// matching `CreatePipe` handle.
pub(crate) fn anon_pipe(
    server_reads: bool,
    overlapped: bool,
    inheritable: bool,
) -> io::Result<(OwnedHandle, OwnedHandle)> {
    const READER: u32 = SYNCHRONIZE | GENERIC_READ | FILE_WRITE_ATTRIBUTES;
    const WRITER: u32 = SYNCHRONIZE | GENERIC_WRITE | FILE_READ_ATTRIBUTES;
    let empty = UNICODE_STRING::default();
    // No name under the pipe file system makes the pipe anonymous, so that
    // no other process can open it.
    let mut attributes = object_attributes(pipe_fs()?, &empty);
    let mut status_block = IO_STATUS_BLOCK::default();
    let (access, share, client_access) = if server_reads {
        (READER, FILE_SHARE_WRITE, WRITER)
    } else {
        (WRITER, FILE_SHARE_READ, READER)
    };
    // Without `FILE_SYNCHRONOUS_IO_*` options, the handle is overlapped.
    let options = if overlapped {
        0
    } else {
        FILE_SYNCHRONOUS_IO_NONALERT
    };
    // 50 ms, relative, in 100 ns units: the default of `CreateNamedPipeW`.
    let timeout: i64 = -500_000;
    let mut server = ptr::null_mut();
    // SAFETY: every pointer is valid for the call. One instance allows only
    // the connection opened below.
    let status = unsafe {
        NtCreateNamedPipeFile(
            &raw mut server,
            access,
            &raw const attributes,
            &raw mut status_block,
            share,
            FILE_CREATE,
            options,
            FILE_PIPE_BYTE_STREAM_TYPE,
            FILE_PIPE_BYTE_STREAM_MODE,
            FILE_PIPE_QUEUE_OPERATION,
            1,
            PIPE_CAPACITY,
            PIPE_CAPACITY,
            &raw const timeout,
        )
    };
    if status < 0 {
        return Err(nt_error(status));
    }
    // SAFETY: the call created a handle that nothing else owns.
    let server = unsafe { OwnedHandle::from_raw_handle(server) };
    // Opening the pipe itself, without a name, connects to it.
    attributes.RootDirectory = server.as_raw_handle();
    if inheritable {
        attributes.Attributes = OBJ_INHERIT;
    }
    let mut client = ptr::null_mut();
    // SAFETY: as above.
    let status = unsafe {
        NtOpenFile(
            &raw mut client,
            client_access,
            &raw const attributes,
            &raw mut status_block,
            0,
            FILE_NON_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT,
        )
    };
    if status < 0 {
        return Err(nt_error(status));
    }
    // SAFETY: the call opened a handle that nothing else owns.
    let client = unsafe { OwnedHandle::from_raw_handle(client) };
    Ok((server, client))
}

/// An end of `io::pipe`, or any handle given as one, which reads and writes
/// synchronously even when the handle was opened for overlapped I/O.
pub(crate) struct Pipe(OwnedHandle);

/// Creates a pipe whose ends are synchronous and not inheritable, as those
/// of `CreatePipe`: `(reader, writer)`.
pub(crate) fn pipe() -> io::Result<(Pipe, Pipe)> {
    let (reader, writer) = anon_pipe(true, false, false)?;
    Ok((Pipe(reader), Pipe(writer)))
}

impl Pipe {
    pub(crate) const fn handle(&self) -> &OwnedHandle {
        &self.0
    }

    pub(crate) fn into_handle(self) -> OwnedHandle {
        self.0
    }

    /// Duplicates the handle, not inheritable.
    pub(crate) fn try_clone(&self) -> io::Result<Self> {
        self.0.try_clone().map(Self)
    }

    /// Reads into `buf`, which only the kernel writes: it initializes the
    /// bytes it reports read and touches no others. A read after every
    /// writer is gone returns 0.
    pub(crate) fn read_uninit(
        &self,
        buf: &mut [MaybeUninit<u8>],
    ) -> io::Result<usize> {
        handle::read(self.0.as_raw_handle(), buf)
    }

    pub(crate) fn write(&self, buf: &[u8]) -> io::Result<usize> {
        handle::write(self.0.as_raw_handle(), buf)
    }

    stream_io!();
    read_to_end_io!();
}

impl From<OwnedHandle> for Pipe {
    fn from(handle: OwnedHandle) -> Self {
        Self(handle)
    }
}

impl fmt::Debug for Pipe {
    /// Formats the pipe as std shows its handle type:
    /// `Handle(OwnedHandle { handle: 0x.. })`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Handle").field(&self.0).finish()
    }
}
