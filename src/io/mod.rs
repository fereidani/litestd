//! Traits, helpers, and type definitions for core I/O functionality.
//!
//! The standard stream handles, `stdin`, `stdout` and `stderr`, need the
//! `stdio` feature as well.
//!
//! # Differences from std
//!
//! - `Stdout` is not line-buffered: like `Stderr` and the `print!` macros,
//!   whose path and lock it shares, each call writes before it returns, so no
//!   output waits for a flush that a `no_std` program has no exit hook to run.
//! - std's `Write::is_write_vectored` is unstable, so [`BufWriter`] and
//!   [`LineWriter`] treat every inner writer as lacking vectored writes, as std
//!   treats every writer defined outside std.
//! - [`copy`] always moves data through a stack buffer, with the same result.
//! - [`Bytes`] reports `(0, None)` from `size_hint`.
//! - Where std panics because a reader or writer reported more bytes than the
//!   buffer holds, litestd clamps the count, and where `Write::write_fmt`
//!   panics because a formatting trait failed, litestd returns an error.
//! - If a reader or writer unwinds out of a buffered type, the buffer contents
//!   are unspecified.

mod buf_read;
mod buffered;
mod cursor;
mod error;
mod impls;
mod io_slice;
mod memchr;
mod pipe;
mod read;
mod seek;
mod stdio;
mod util;
mod write;

// For files, sockets and pipes, whose data only the OS writes, to read to
// the end into uninitialized memory.
#[cfg(all(unix, feature = "command"))]
pub(crate) use self::read::UninitReadToEnd;
// The OS readers lend it initialized buffers; wasm32-unknown-unknown has
// none.
#[cfg(not(target_os = "unknown"))]
pub(crate) use self::read::as_uninit;
#[cfg(any(not(target_os = "unknown"), feature = "fs"))]
pub(crate) use self::read::read_to_end_uninit;
#[cfg(feature = "fs")]
pub(crate) use self::read::read_to_string_uninit;
#[cfg(feature = "stdio")]
pub use self::stdio::{
    Stderr, StderrLock, Stdin, StdinLock, Stdout, StdoutLock, stderr, stdin,
    stdout,
};
pub use self::{
    buf_read::BufRead,
    buffered::{
        BufReader, BufWriter, IntoInnerError, LineWriter, WriterPanicked,
    },
    cursor::Cursor,
    error::{Error, ErrorKind, Result},
    io_slice::{IoSlice, IoSliceMut},
    pipe::{PipeReader, PipeWriter, pipe},
    read::{Read, read_to_string},
    seek::{Seek, SeekFrom},
    stdio::IsTerminal,
    util::{
        Bytes, Chain, Empty, Lines, Repeat, Sink, Split, Take, copy, empty,
        repeat, sink,
    },
    write::Write,
};
pub(crate) use self::{
    error::{SimpleMessage, const_error},
    read::{DEFAULT_BUF_SIZE, append_to_string},
};

pub mod prelude {
    //! The I/O prelude: a glob import of it brings the I/O traits into scope.

    pub use super::{BufRead, Read, Seek, Write};
}
