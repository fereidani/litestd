//! The standard output streams behind the `print!` family of macros and
//! `io::stdout` and `io::stderr`, and the panic handler.
//!
//! Output is unbuffered. Each call locks its stream, gathers its output in
//! the stream's 1 KiB buffer and writes it out before returning, in one
//! write when it fits: nothing needs flushing, nothing is lost when the
//! process aborts, and calls from different threads do not interleave. The
//! lock is reentrant, so a formatting trait implementation may print; its
//! output lands where std's would. As in std, the macros panic when a write
//! fails, and a stream without a valid handle discards what they print.

#[cfg(feature = "stdio")]
pub use self::print::{_eprint, _eprintln, _print, _println};
#[cfg(all(feature = "stdio", feature = "io"))]
pub(crate) use self::{
    raw::WriteError,
    stream::{Output, OutputGuard},
};

/// Writing to a standard stream, for the streams and for the panic handler.
#[cfg(any(
    feature = "stdio",
    all(
        feature = "panic-location",
        not(any(
            test,
            feature = "test-with-std",
            feature = "custom-panic-handler"
        ))
    )
))]
mod raw {
    use crate::sys::stdio::{self as sys_stdio, Stream};

    /// Why a write to a standard stream failed.
    #[cfg_attr(
        not(all(feature = "stdio", feature = "io")),
        allow(dead_code, reason = "only the `io` handles report errors")
    )]
    #[derive(Clone, Copy)]
    pub(crate) enum WriteError {
        /// The OS failed with this error code.
        Os(i32),
        /// The stream accepted no bytes.
        Zero,
        /// A formatting trait implementation failed.
        #[cfg(feature = "stdio")]
        Format,
    }

    /// Writes all of `buf` to `stream`, retrying after partial writes and
    /// interruptions. A stream without a valid handle discards the bytes, as
    /// in std.
    #[inline(never)]
    pub(super) fn write_all(
        stream: Stream,
        mut buf: &[u8],
    ) -> Result<(), WriteError> {
        // Every pass consumes at least one byte, returns, or retries after an
        // interruption, so the loop ends once `buf` is empty.
        while !buf.is_empty() {
            match sys_stdio::write(stream, buf) {
                Ok(0) => return Err(WriteError::Zero),
                // The OS never reports more bytes than it was given; should
                // it, the write counts as complete rather than indexing out
                // of bounds.
                Ok(n) => buf = buf.get(n..).unwrap_or_default(),
                Err(code) if sys_stdio::is_interrupted(code) => {}
                Err(code) if sys_stdio::is_ebadf(code) => return Ok(()),
                Err(code) => return Err(WriteError::Os(code)),
            }
        }
        Ok(())
    }
}

/// The standard output streams and their reentrant locks.
#[cfg(feature = "stdio")]
mod stream {
    use core::{
        cell::{Cell, UnsafeCell},
        fmt,
        marker::PhantomData,
        sync::atomic::{AtomicUsize, Ordering::Relaxed},
    };

    use super::raw::{WriteError, write_all};
    use crate::{
        sync::raw_mutex::RawMutex,
        sys::{
            os,
            stdio::{self as sys_stdio, Stream},
        },
    };

    /// The capacity of a stream's buffer. Longer output is written in
    /// pieces.
    const CAPACITY: usize = 1024;

    /// A standard output stream: the OS stream, and the lock and buffer of
    /// the process's writes to it.
    #[derive(Clone, Copy)]
    pub(crate) struct Output {
        stream: Stream,
        state: &'static State,
    }

    // Zero-initialized, so that they take no space in the binary.
    static STDOUT_STATE: State = State::new();
    static STDERR_STATE: State = State::new();

    /// A lock that its holder may take again, as std's `ReentrantLock`, and
    /// the buffer that gathers the output of one call while it holds it.
    ///
    /// `mutex` excludes other threads; `owner` names the holder by
    /// `sys_stdio::thread_id`, unique among live threads and never 0, so a
    /// thread tells that it holds the lock without thread-local storage.
    struct State {
        mutex: RawMutex,
        /// The holder's thread id, or 0 while the lock is free.
        owner: AtomicUsize,
        /// How often the holder has taken the lock.
        count: Cell<u32>,
        buf: UnsafeCell<Buffer>,
    }

    // SAFETY: only the thread holding the lock accesses `count` and `buf`,
    // and `mutex` grants the lock to one thread at a time; its `Acquire` and
    // `Release` order the accesses of successive holders.
    unsafe impl Sync for State {}

    impl State {
        const fn new() -> Self {
            Self {
                mutex: RawMutex::new(),
                owner: AtomicUsize::new(0),
                count: Cell::new(0),
                buf: UnsafeCell::new(Buffer {
                    len: 0,
                    bytes: [0; CAPACITY],
                }),
            }
        }
    }

    impl Output {
        /// Standard output.
        #[inline]
        pub(crate) fn stdout() -> Self {
            Self {
                stream: sys_stdio::STDOUT,
                state: &STDOUT_STATE,
            }
        }

        /// Standard error.
        #[inline]
        pub(crate) fn stderr() -> Self {
            Self {
                stream: sys_stdio::STDERR,
                state: &STDERR_STATE,
            }
        }

        /// Whether this is the standard output.
        pub(crate) fn is_stdout(self) -> bool {
            core::ptr::eq(self.state, &raw const STDOUT_STATE)
        }

        /// Locks the stream for the calling thread, which may hold it
        /// already, blocking while another thread holds it.
        pub(crate) fn lock(self) -> OutputGuard {
            let state = self.state;
            let this = sys_stdio::thread_id();
            // Only this thread stores its id, and it clears it before it
            // releases the mutex, so it reads its own id exactly while it
            // holds the lock: `Relaxed` suffices.
            if state.owner.load(Relaxed) == this {
                // Only leaked guards nest this deep. Wrapping would let two
                // threads in, so it aborts, where std panics.
                let Some(count) = state.count.get().checked_add(1) else {
                    os::abort()
                };
                state.count.set(count);
            } else {
                state.mutex.lock();
                state.owner.store(this, Relaxed);
                state.count.set(1);
            }
            OutputGuard {
                output: self,
                _not_send: PhantomData,
            }
        }
    }

    /// Proof that the calling thread holds a stream's lock, which it
    /// releases once when dropped. It cannot leave the thread.
    pub(crate) struct OutputGuard {
        output: Output,
        _not_send: PhantomData<*const ()>,
    }

    impl Drop for OutputGuard {
        fn drop(&mut self) {
            let state = self.output.state;
            let count = state.count.get();
            debug_assert!(count > 0, "a guard exists");
            state.count.set(count - 1);
            if count == 1 {
                state.owner.store(0, Relaxed);
                // SAFETY: this thread took the mutex when its count went to
                // 1, and gives up the buffer and the count with it.
                unsafe { state.mutex.unlock() };
            }
        }
    }

    impl OutputGuard {
        /// Runs `f` on the stream's buffer.
        ///
        /// `f` must not print: it runs no caller's code, such as a
        /// formatting trait implementation, and does not call this again.
        fn with_buf<R>(&self, f: impl FnOnce(&mut Buffer, Stream) -> R) -> R {
            // SAFETY: the pointer comes from a static's `UnsafeCell`, so it
            // is valid and aligned. The calling thread holds the lock, as
            // the guard proves, so no other thread accesses the buffer; on
            // this thread, the borrow ends before anything else could borrow
            // it, as `f` does not print.
            let buf = unsafe { &mut *self.output.state.buf.get() };
            f(buf, self.output.stream)
        }

        /// Appends `bytes` to the buffer, writing it out whenever it fills.
        pub(crate) fn push(&self, bytes: &[u8]) -> Result<(), WriteError> {
            self.with_buf(|buf, stream| buf.push(stream, bytes))
        }

        /// Writes out and empties the buffer.
        pub(crate) fn flush(&self) -> Result<(), WriteError> {
            self.with_buf(Buffer::flush)
        }

        /// Writes `s` and, if `newline` is set, a newline: one write when
        /// they fit in the buffer.
        pub(crate) fn write_str(
            &self,
            s: &str,
            newline: bool,
        ) -> Result<(), WriteError> {
            self.with_buf(|buf, stream| {
                buf.push(stream, s.as_bytes())?;
                if newline {
                    buf.push(stream, b"\n")?;
                }
                buf.flush(stream)
            })
        }

        /// Formats `args`, adds a newline if `newline` is set and formatting
        /// succeeded, and writes it all out. What was gathered is written
        /// even after a failure, as std writes each piece as it comes.
        pub(crate) fn write_fmt(
            &self,
            args: fmt::Arguments<'_>,
            newline: bool,
        ) -> Result<(), WriteError> {
            let mut out = Adapter {
                guard: self,
                error: None,
            };
            let result = match (fmt::write(&mut out, args), out.error) {
                (_, Some(error)) => Err(error),
                (Err(fmt::Error), None) => Err(WriteError::Format),
                (Ok(()), None) if newline => self.push(b"\n"),
                (Ok(()), None) => Ok(()),
            };
            let flushed = self.flush();
            result.and(flushed)
        }

        /// Writes out the buffer, then writes once from `bytes`, as
        /// `Write::write`: returns how many bytes of `bytes` were written.
        #[cfg(feature = "io")]
        pub(crate) fn write(&self, bytes: &[u8]) -> Result<usize, WriteError> {
            self.flush()?;
            sys_stdio::write(self.output.stream, bytes).map_err(WriteError::Os)
        }
    }

    /// Forwards the pieces of a formatted message to a locked stream,
    /// keeping the first write error. It holds no borrow of the buffer
    /// between pieces, while formatting trait implementations run.
    struct Adapter<'a> {
        guard: &'a OutputGuard,
        error: Option<WriteError>,
    }

    impl fmt::Write for Adapter<'_> {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            self.guard.push(s.as_bytes()).map_err(|error| {
                self.error = Some(error);
                fmt::Error
            })
        }
    }

    /// The bytes that one call gathers before they are written out.
    struct Buffer {
        /// The number of filled bytes at the front of `bytes`.
        len: usize,
        bytes: [u8; CAPACITY],
    }

    impl Buffer {
        /// Appends `bytes`, writing out the buffer first if they do not fit,
        /// or writing them out directly if they do not fit even then.
        #[inline(never)]
        fn push(
            &mut self,
            stream: Stream,
            bytes: &[u8],
        ) -> Result<(), WriteError> {
            if !self.try_append(bytes) {
                self.flush(stream)?;
                if !self.try_append(bytes) {
                    return write_all(stream, bytes);
                }
            }
            Ok(())
        }

        /// Appends `bytes` if they fit in the free space.
        fn try_append(&mut self, bytes: &[u8]) -> bool {
            let free = self.bytes.get_mut(self.len..).unwrap_or_default();
            let Some(dst) = free.get_mut(..bytes.len()) else {
                return false;
            };
            dst.copy_from_slice(bytes);
            // `dst` ends within `bytes`, so `len` stays at most `CAPACITY`.
            self.len += bytes.len();
            true
        }

        /// Writes out the filled bytes and empties the buffer.
        fn flush(&mut self, stream: Stream) -> Result<(), WriteError> {
            let filled = self.bytes.get(..self.len).unwrap_or_default();
            let result = write_all(stream, filled);
            self.len = 0;
            result
        }
    }
}

/// The print machinery behind the macros.
#[cfg(feature = "stdio")]
mod print {
    #![allow(
        clippy::inline_always,
        reason = "the check must fold at each call"
    )]

    use core::fmt;

    use super::stream::Output;

    /// Prints a message that needs no formatting, plus a newline if
    /// `newline` is set.
    #[inline(never)]
    fn print_str(output: Output, s: &str, newline: bool) {
        // The guard is dropped at the end of the statement, so the stream is
        // unlocked before any panic.
        let result = output.lock().write_str(s, newline);
        if result.is_err() {
            failed(output);
        }
    }

    /// Formats and prints a message, plus a newline if `newline` is set.
    #[inline(never)]
    fn print_fmt(output: Output, args: fmt::Arguments<'_>, newline: bool) {
        let result = output.lock().write_fmt(args, newline);
        if result.is_err() {
            failed(output);
        }
    }

    /// Panics because printing to `output` failed, as std's `print!` and
    /// `eprint!` families document; std adds the error to the message.
    #[cold]
    #[inline(never)]
    #[track_caller]
    #[allow(clippy::panic, reason = "std documents this panic")]
    #[allow(clippy::manual_assert, reason = "one message per stream")]
    fn failed(output: Output) -> ! {
        if output.is_stdout() {
            panic!("failed printing to stdout");
        }
        panic!("failed printing to stderr")
    }

    /// Dispatches on whether `args` needs formatting. Inlined into each macro
    /// call, where the check folds away, so a program that prints only
    /// literals never links the formatting machinery.
    #[inline(always)]
    fn print(output: Output, args: fmt::Arguments<'_>, newline: bool) {
        match args.as_str() {
            Some(s) => print_str(output, s, newline),
            None => print_fmt(output, args, newline),
        }
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn _print(args: fmt::Arguments<'_>) {
        print(Output::stdout(), args, false);
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn _println(args: fmt::Arguments<'_>) {
        print(Output::stdout(), args, true);
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn _eprint(args: fmt::Arguments<'_>) {
        print(Output::stderr(), args, false);
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn _eprintln(args: fmt::Arguments<'_>) {
        print(Output::stderr(), args, true);
    }

    /// Prints to the standard output.
    ///
    /// Unlike std's, the output is unbuffered, so there is nothing to flush.
    ///
    /// # Panics
    ///
    /// Panics if writing to the standard output fails, as in std.
    #[macro_export]
    macro_rules! print {
        ($($arg:tt)*) => {{
            $crate::_print($crate::format_args!($($arg)*));
        }};
    }

    /// Prints to the standard output, with a newline.
    ///
    /// The line is one write when it fits in 1 KiB.
    ///
    /// # Panics
    ///
    /// Panics if writing to the standard output fails, as in std.
    #[macro_export]
    macro_rules! println {
        () => {
            $crate::print!("\n")
        };
        ($($arg:tt)*) => {{
            $crate::_println($crate::format_args!($($arg)*));
        }};
    }

    /// Prints to the standard error, like [`print!`].
    ///
    /// # Panics
    ///
    /// Panics if writing to the standard error fails, as in std.
    #[macro_export]
    macro_rules! eprint {
        ($($arg:tt)*) => {{
            $crate::_eprint($crate::format_args!($($arg)*));
        }};
    }

    /// Prints to the standard error, with a newline, like
    /// [`println!`](crate::println).
    ///
    /// # Panics
    ///
    /// Panics if writing to the standard error fails, as in std.
    #[macro_export]
    macro_rules! eprintln {
        () => {
            $crate::eprint!("\n")
        };
        ($($arg:tt)*) => {{
            $crate::_eprintln($crate::format_args!($($arg)*));
        }};
    }

    /// Prints and returns the value of a given expression for quick and dirty
    /// debugging.
    ///
    /// Writes the location, the expression and its pretty [`Debug`] value to
    /// the standard error, and returns the value; several expressions give a
    /// tuple.
    ///
    /// [`Debug`]: crate::fmt::Debug
    #[macro_export]
    macro_rules! dbg {
        () => {
            $crate::eprintln!(
                "[{}:{}:{}]",
                $crate::file!(),
                $crate::line!(),
                $crate::column!()
            )
        };
        ($val:expr $(,)?) => {
            // The `match` keeps temporaries in `$val` alive until printed.
            match $val {
                tmp => {
                    $crate::eprintln!(
                        "[{}:{}:{}] {} = {:#?}",
                        $crate::file!(),
                        $crate::line!(),
                        $crate::column!(),
                        $crate::stringify!($val),
                        &&tmp as &dyn $crate::fmt::Debug,
                    );
                    tmp
                }
            }
        };
        ($($val:expr),+ $(,)?) => {
            ($($crate::dbg!($val)),+,)
        };
    }
}

/// The panic handler: every panic terminates the process.
///
/// It makes std and litestd mutually exclusive, as the crate documentation
/// explains, and is compiled out under `cfg(test)`, `test-with-std` and
/// `custom-panic-handler`. With `panic-location` it first writes
/// `panicked at FILE:LINE:COLUMN` to the standard error, which needs none of
/// the formatting machinery. With `panic-message` it writes that, a colon
/// and the message instead, as std does, through the formatting machinery.
#[cfg(not(any(
    test,
    feature = "test-with-std",
    feature = "custom-panic-handler"
)))]
mod panic_handler {
    use core::panic::PanicInfo;

    use crate::sys::os;

    #[panic_handler]
    fn panic(info: &PanicInfo<'_>) -> ! {
        #[cfg(feature = "panic-message")]
        message::report(info);
        #[cfg(all(feature = "panic-location", not(feature = "panic-message")))]
        if let Some(location) = info.location() {
            location::report(location);
        }
        #[cfg(not(feature = "panic-location"))]
        let _ = info;
        os::abort()
    }

    #[cfg(feature = "panic-message")]
    mod message {
        use core::{
            fmt::{self, Write},
            panic::PanicInfo,
            sync::atomic::{AtomicBool, Ordering::Relaxed},
        };

        use crate::{stdio::raw::write_all, sys::stdio};

        /// The standard error, written without its lock, which its holder
        /// may never release.
        struct Stderr;

        impl Write for Stderr {
            fn write_str(&mut self, s: &str) -> fmt::Result {
                write_all(stdio::STDERR, s.as_bytes()).map_err(|_| fmt::Error)
            }
        }

        /// Writes `panicked at FILE:LINE:COLUMN:`, the message and a newline
        /// to the standard error, as std's panic hook does without the
        /// thread's name. Only the first panic reports: a panic in a
        /// formatting trait implementation that the message runs, or in
        /// another thread meanwhile, goes straight to the abort.
        pub(super) fn report(info: &PanicInfo<'_>) {
            static REPORTING: AtomicBool = AtomicBool::new(false);
            // Relaxed: only which panic comes first matters.
            if REPORTING.swap(true, Relaxed) {
                return;
            }
            // The report cannot finish a character that a write left for the
            // next one, as on a Windows console.
            stdio::reset(stdio::STDERR);
            // The process aborts next, so a failed write changes nothing.
            let _ = writeln!(Stderr, "{info}");
        }
    }

    #[cfg(all(feature = "panic-location", not(feature = "panic-message")))]
    mod location {
        use core::panic::Location;

        use crate::{stdio::raw::write_all, sys::stdio};

        /// Writes `panicked at FILE:LINE:COLUMN` and a newline to the
        /// standard error. It cannot panic: the text is copied, never
        /// formatted, and every index is checked.
        pub(super) fn report(location: &Location<'_>) {
            let mut tail = Tail::new();
            for n in [location.column(), location.line()] {
                tail.push_decimal_front(n);
                tail.push_front(b':');
            }
            let out = stdio::STDERR;
            // The report cannot finish a character that a write left for the
            // next one, as on a Windows console.
            stdio::reset(out);
            // The process aborts next, so a failed write changes nothing.
            // The stream is not locked: its holder may never unlock it.
            let _ = write_all(out, b"panicked at ")
                .and_then(|()| write_all(out, location.file().as_bytes()))
                .and_then(|()| write_all(out, tail.filled()));
        }

        /// `:LINE:COLUMN` and a newline, built back to front.
        struct Tail {
            /// Two ten-digit numbers, two colons and a newline.
            buf: [u8; 23],
            /// The first filled byte: `buf[start..]` is filled.
            start: usize,
        }

        impl Tail {
            /// Returns a tail holding just the newline.
            const fn new() -> Self {
                let mut buf = [0; 23];
                buf[22] = b'\n';
                Self { buf, start: 22 }
            }

            fn push_front(&mut self, byte: u8) {
                let Some(index) = self.start.checked_sub(1) else {
                    return;
                };
                if let Some(slot) = self.buf.get_mut(index) {
                    *slot = byte;
                    self.start = index;
                }
            }

            fn push_decimal_front(&mut self, mut n: u32) {
                // `u32::MAX` has ten digits.
                for _ in 0..10 {
                    // `n % 10 < 10`, so the digit fits in a `u8`.
                    #[allow(clippy::cast_possible_truncation)]
                    let digit = (n % 10) as u8;
                    self.push_front(b'0' + digit);
                    n /= 10;
                    if n == 0 {
                        break;
                    }
                }
            }

            fn filled(&self) -> &[u8] {
                self.buf.get(self.start..).unwrap_or_default()
            }
        }
    }
}

/// The unwinder's personality routine, which the precompiled `core` and
/// `alloc` reference. Every `no_std` binary gets it, so binaries must not
/// define their own. It never runs under `panic = "abort"`; if a foreign
/// unwinder reached it, aborting is the only sound answer. WebAssembly's
/// precompiled crates never unwind, and need none.
#[cfg(not(any(test, feature = "test-with-std", target_family = "wasm")))]
#[unsafe(no_mangle)]
extern "C" fn rust_eh_personality() -> ! {
    crate::sys::os::reference_unwinder();
    crate::sys::os::abort()
}
