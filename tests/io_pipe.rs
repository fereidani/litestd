//! `litestd::io::pipe` against `std::io::pipe`: data through the ends of
//! either library, EOF and broken pipes, `try_clone`, the conversions to and
//! from descriptors and handles, `Debug` and the traits, and pipe ends as a
//! child's streams. Miri runs everything but the child processes.

// WASI and wasm32-unknown-unknown have no pipes.
#![cfg(all(feature = "io", any(unix, windows)))]
#![allow(clippy::unwrap_used, reason = "a failure fails the test")]
#![allow(clippy::incompatible_msrv, reason = "std's pipes need Rust 1.87")]

use core::{
    fmt::Debug,
    panic::{RefUnwindSafe, UnwindSafe},
    time::Duration,
};
use std::{
    io::{Read as _, Write as _},
    thread,
};

use litestd::io::{
    self as lite, IoSlice, IoSliceMut, IsTerminal, Read as LiteRead,
    Write as LiteWrite,
};

/// Moves a litestd reader into std, keeping the descriptor or handle.
fn to_std_reader(reader: lite::PipeReader) -> std::io::PipeReader {
    #[cfg(unix)]
    {
        use std::os::fd::FromRawFd;

        use litestd::os::fd::IntoRawFd;
        // SAFETY: the descriptor comes from a reader that gave it up.
        unsafe { std::io::PipeReader::from_raw_fd(reader.into_raw_fd()) }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::FromRawHandle;

        use litestd::os::windows::io::IntoRawHandle;
        // SAFETY: the handle comes from a reader that gave it up.
        unsafe {
            std::io::PipeReader::from_raw_handle(reader.into_raw_handle())
        }
    }
}

/// Moves a litestd writer into std, keeping the descriptor or handle.
fn to_std_writer(writer: lite::PipeWriter) -> std::io::PipeWriter {
    #[cfg(unix)]
    {
        use std::os::fd::FromRawFd;

        use litestd::os::fd::IntoRawFd;
        // SAFETY: the descriptor comes from a writer that gave it up.
        unsafe { std::io::PipeWriter::from_raw_fd(writer.into_raw_fd()) }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::FromRawHandle;

        use litestd::os::windows::io::IntoRawHandle;
        // SAFETY: the handle comes from a writer that gave it up.
        unsafe {
            std::io::PipeWriter::from_raw_handle(writer.into_raw_handle())
        }
    }
}

/// Moves a std reader into litestd, keeping the descriptor or handle.
fn to_lite_reader(reader: std::io::PipeReader) -> lite::PipeReader {
    #[cfg(unix)]
    {
        use std::os::fd::IntoRawFd;

        use litestd::os::fd::FromRawFd;
        // SAFETY: the descriptor comes from a reader that gave it up.
        unsafe { lite::PipeReader::from_raw_fd(reader.into_raw_fd()) }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::IntoRawHandle;

        use litestd::os::windows::io::FromRawHandle;
        // SAFETY: the handle comes from a reader that gave it up.
        unsafe { lite::PipeReader::from_raw_handle(reader.into_raw_handle()) }
    }
}

/// The raw descriptor of a pipe end, and whether a child would inherit it.
#[cfg(unix)]
fn raw(end: &impl litestd::os::fd::AsRawFd) -> (usize, bool) {
    // SAFETY: `F_GETFD` touches no memory.
    let flags = unsafe { libc::fcntl(end.as_raw_fd(), libc::F_GETFD) };
    assert!(flags >= 0);
    let fd = usize::try_from(end.as_raw_fd()).unwrap();
    (fd, flags & libc::FD_CLOEXEC == 0)
}

/// The raw handle of a pipe end, and whether a child would inherit it.
#[cfg(windows)]
fn raw(end: &impl litestd::os::windows::io::AsRawHandle) -> (usize, bool) {
    use windows_sys::Win32::Foundation::{
        GetHandleInformation, HANDLE_FLAG_INHERIT,
    };
    let mut flags = 0;
    // SAFETY: the handle is open, and `flags` is valid for writes.
    let ok =
        unsafe { GetHandleInformation(end.as_raw_handle(), &raw mut flags) };
    assert_ne!(ok, 0);
    (end.as_raw_handle().addr(), flags & HANDLE_FLAG_INHERIT != 0)
}

#[test]
fn data_flows_between_the_ends_of_both_libraries() {
    let (reader, mut writer) = lite::pipe().unwrap();
    let mut reader = to_std_reader(reader);
    writer.write_all(b"from litestd").unwrap();
    drop(writer);
    let mut text = String::new();
    reader.read_to_string(&mut text).unwrap();
    assert_eq!(text, "from litestd");

    let (reader, mut writer) = std::io::pipe().unwrap();
    let mut reader = to_lite_reader(reader);
    writer.write_all(b"from std").unwrap();
    drop(writer);
    let mut text = String::new();
    reader.read_to_string(&mut text).unwrap();
    assert_eq!(text, "from std");
}

/// Two pipes carry messages both ways between two threads.
#[test]
#[cfg_attr(miri, ignore = "Miri deadlocks here with std's pipes too")]
fn a_pair_of_pipes_carries_data_both_ways() {
    let (mut ping_rx, mut ping_tx) = lite::pipe().unwrap();
    let (mut pong_rx, mut pong_tx) = lite::pipe().unwrap();
    let echo = thread::spawn(move || {
        let mut buf = [0; 4];
        // Echoes until the pinging side closes its pipe.
        while ping_rx.read_exact(&mut buf).is_ok() {
            pong_tx.write_all(&buf).unwrap();
        }
    });
    for i in 0..16u32 {
        ping_tx.write_all(&i.to_le_bytes()).unwrap();
        let mut buf = [0; 4];
        pong_rx.read_exact(&mut buf).unwrap();
        assert_eq!(u32::from_le_bytes(buf), i);
    }
    drop(ping_tx);
    echo.join().unwrap();
    let mut rest = Vec::new();
    assert_eq!(pong_rx.read_to_end(&mut rest).unwrap(), 0);
}

#[test]
fn reads_and_writes_through_shared_references() {
    let (reader, writer) = lite::pipe().unwrap();
    (&writer).write_all(b"shared ").unwrap();
    let bufs = [
        IoSlice::new(b"vec"),
        IoSlice::new(b""),
        IoSlice::new(b"tor"),
    ];
    let n = (&writer).write_vectored(&bufs).unwrap();
    assert!(n > 0);
    (&writer).write_all(&b"vector"[n..]).unwrap();
    (&writer).flush().unwrap();
    drop(writer);
    let (mut first, mut second) = ([0; 3], [0; 4]);
    let mut bufs = [IoSliceMut::new(&mut first), IoSliceMut::new(&mut second)];
    let n = (&reader).read_vectored(&mut bufs).unwrap();
    assert!(n > 0);
    let mut all: Vec<u8> =
        first.iter().chain(&second).copied().take(n).collect();
    (&reader).read_to_end(&mut all).unwrap();
    assert_eq!(all, b"shared vector");
}

/// `read_to_end` and `read_to_string` read large amounts, keep what the
/// vector held, and validate UTF-8 as std does.
#[test]
fn reading_to_the_end_matches_std() {
    let len = if cfg!(miri) { 5000 } else { 3 << 20 };
    let data: Vec<u8> =
        (0..len).map(|i| u8::try_from(i % 251).unwrap()).collect();
    let (mut reader, mut writer) = lite::pipe().unwrap();
    let sent = data.clone();
    let feeder = thread::spawn(move || writer.write_all(&sent).unwrap());
    let mut all = b"kept".to_vec();
    assert_eq!(reader.read_to_end(&mut all).unwrap(), len);
    feeder.join().unwrap();
    assert_eq!(&all[..4], b"kept");
    assert_eq!(&all[4..], &data[..]);

    for input in [&b"valid \xc3\xa9"[..], b"invalid \xff"] {
        let (mut lite_rx, mut lite_tx) = lite::pipe().unwrap();
        let (mut std_rx, mut std_tx) = std::io::pipe().unwrap();
        lite_tx.write_all(input).unwrap();
        std_tx.write_all(input).unwrap();
        drop((lite_tx, std_tx));
        let (mut lite_text, mut std_text) =
            (String::from("a"), String::from("a"));
        let lite = lite_rx.read_to_string(&mut lite_text);
        let std = std_rx.read_to_string(&mut std_text);
        assert_eq!(lite_text, std_text);
        match (lite, std) {
            (Ok(a), Ok(b)) => assert_eq!(a, b),
            (Err(a), Err(b)) => {
                assert_eq!(
                    format!("{:?}", a.kind()),
                    format!("{:?}", b.kind())
                );
                assert_eq!(a.to_string(), b.to_string());
            }
            (a, b) => panic!("litestd {a:?}, std {b:?}"),
        }
    }
}

/// Reads return EOF only once every copy of the writer is closed.
#[test]
fn eof_waits_for_every_writer() {
    let (mut reader, writer) = lite::pipe().unwrap();
    let clone = writer.try_clone().unwrap();
    drop(writer);
    let late = thread::spawn(move || {
        thread::sleep(Duration::from_millis(20));
        (&clone).write_all(b"late").unwrap();
    });
    let mut data = Vec::new();
    reader.read_to_end(&mut data).unwrap();
    late.join().unwrap();
    assert_eq!(data, b"late");
    assert_eq!(reader.read(&mut [0; 8]).unwrap(), 0);
}

/// Writes one byte at a time until a write fails, as one does once no
/// process holds the read end: a child that another test starts holds a
/// copy of every descriptor until it runs its program.
#[allow(clippy::panic, reason = "fails the test")]
fn write_until_error<E>(mut write: impl FnMut(&[u8]) -> Result<usize, E>) -> E {
    // At most ten seconds, far longer than any child takes to start.
    for _ in 0..10_000 {
        match write(b"x") {
            Err(e) => return e,
            Ok(_) => thread::sleep(Duration::from_millis(1)),
        }
    }
    panic!("writes kept succeeding");
}

/// Writes fail once every copy of the reader is closed, with std's error;
/// std's runtime ignores `SIGPIPE` in this test binary.
#[test]
fn writes_fail_once_every_reader_is_gone() {
    let (reader, mut writer) = lite::pipe().unwrap();
    let clone = reader.try_clone().unwrap();
    drop(reader);
    writer.write_all(b"still read").unwrap();
    drop(clone);
    let lite = write_until_error(|buf| writer.write(buf));
    let (reader, mut writer) = std::io::pipe().unwrap();
    drop(reader);
    let std = write_until_error(|buf| writer.write(buf));
    assert_eq!(lite.kind(), lite::ErrorKind::BrokenPipe);
    assert_eq!(format!("{:?}", lite.kind()), format!("{:?}", std.kind()));
    assert_eq!(lite.raw_os_error(), std.raw_os_error());
    assert_eq!(lite.to_string(), std.to_string());
}

#[test]
fn clones_share_the_pipe_and_no_end_is_inheritable() {
    let (reader, writer) = lite::pipe().unwrap();
    let reader2 = reader.try_clone().unwrap();
    let writer2 = writer.try_clone().unwrap();
    let ends = [raw(&reader), raw(&reader2), raw(&writer), raw(&writer2)];
    assert_ne!(ends[0].0, ends[1].0);
    assert_ne!(ends[2].0, ends[3].0);
    assert!(
        ends.iter().all(|&(_, inheritable)| !inheritable),
        "{ends:?}"
    );
    (&writer2).write_all(b"ab").unwrap();
    drop(writer2);
    let mut byte = [0; 1];
    (&reader).read_exact(&mut byte).unwrap();
    assert_eq!(&byte, b"a");
    (&reader2).read_exact(&mut byte).unwrap();
    assert_eq!(&byte, b"b");
    drop(writer);
    assert_eq!((&reader2).read(&mut byte).unwrap(), 0);
}

#[cfg(unix)]
#[test]
fn ends_convert_to_and_from_descriptors() {
    use litestd::os::fd::{AsFd, AsRawFd, FromRawFd, IntoRawFd, OwnedFd};

    let (reader, writer) = lite::pipe().unwrap();
    let (rfd, wfd) = (reader.as_raw_fd(), writer.as_raw_fd());
    assert_eq!(reader.as_fd().as_raw_fd(), rfd);
    let reader = lite::PipeReader::from(OwnedFd::from(reader));
    // SAFETY: the descriptor comes from a writer that gave it up.
    let mut writer =
        unsafe { lite::PipeWriter::from_raw_fd(writer.into_raw_fd()) };
    assert_eq!((reader.as_raw_fd(), writer.as_raw_fd()), (rfd, wfd));
    writer.write_all(b"converted").unwrap();
    drop(writer);
    let mut reader = to_std_reader(reader);
    let mut text = String::new();
    reader.read_to_string(&mut text).unwrap();
    assert_eq!(text, "converted");
}

#[cfg(windows)]
#[test]
fn ends_convert_to_and_from_handles() {
    use litestd::os::windows::io::{
        AsHandle, AsRawHandle, FromRawHandle, IntoRawHandle, OwnedHandle,
    };

    let (reader, writer) = lite::pipe().unwrap();
    let (rh, wh) = (reader.as_raw_handle(), writer.as_raw_handle());
    assert_eq!(reader.as_handle().as_raw_handle(), rh);
    let reader = lite::PipeReader::from(OwnedHandle::from(reader));
    // SAFETY: the handle comes from a writer that gave it up.
    let mut writer =
        unsafe { lite::PipeWriter::from_raw_handle(writer.into_raw_handle()) };
    assert_eq!((reader.as_raw_handle(), writer.as_raw_handle()), (rh, wh));
    writer.write_all(b"converted").unwrap();
    drop(writer);
    let mut reader = to_std_reader(reader);
    let mut text = String::new();
    reader.read_to_string(&mut text).unwrap();
    assert_eq!(text, "converted");
}

#[test]
fn debug_output_matches_std() {
    let (reader, writer) = lite::pipe().unwrap();
    let lite = format!("{reader:?} {writer:?} {reader:#?}");
    let (reader, writer) = (to_std_reader(reader), to_std_writer(writer));
    assert_eq!(lite, format!("{reader:?} {writer:?} {reader:#?}"));
}

/// Evaluates to whether `$ty` implements `$tr`. The inherent constant takes
/// precedence over the trait's, but exists only where the bound holds.
macro_rules! implements {
    ($ty:ty: $tr:path) => {{
        struct Probe<T: ?Sized>(core::marker::PhantomData<T>);
        #[allow(dead_code)]
        trait Fallback {
            const YES: bool = false;
        }
        impl<T: ?Sized> Fallback for Probe<T> {}
        #[allow(dead_code)]
        impl<T: ?Sized + $tr> Probe<T> {
            const YES: bool = true;
        }
        <Probe<$ty>>::YES
    }};
}

/// Asserts that `$lite` and `$std` agree on each trait, given as a pair of
/// litestd's and std's trait where they differ.
macro_rules! same_traits {
    ($lite:ty, $std:ty: $($tr:path),+ ; $($lt:path = $st:path),+) => {
        $(assert_eq!(
            implements!($lite: $tr),
            implements!($std: $tr),
            "{} disagree on {}", stringify!($lite), stringify!($tr),
        );)+
        $(assert_eq!(
            implements!($lite: $lt),
            implements!($std: $st),
            "{} disagree on {}", stringify!($lite), stringify!($lt),
        );)+
    };
}

#[test]
fn ends_have_std_s_traits() {
    same_traits!(lite::PipeReader, std::io::PipeReader:
        Send, Sync, Unpin, UnwindSafe, RefUnwindSafe, Debug, Clone, Default;
        LiteRead = std::io::Read, LiteWrite = std::io::Write,
        IsTerminal = std::io::IsTerminal);
    same_traits!(lite::PipeWriter, std::io::PipeWriter:
        Send, Sync, Unpin, UnwindSafe, RefUnwindSafe, Debug, Clone, Default;
        LiteRead = std::io::Read, LiteWrite = std::io::Write,
        IsTerminal = std::io::IsTerminal);
    same_traits!(&lite::PipeReader, &std::io::PipeReader: Send, Sync;
        LiteRead = std::io::Read, LiteWrite = std::io::Write);
    same_traits!(&lite::PipeWriter, &std::io::PipeWriter: Send, Sync;
        LiteRead = std::io::Read, LiteWrite = std::io::Write);
}

/// Pipe ends as a child's streams, through litestd's `Command` and std's.
#[cfg(all(feature = "command", not(miri)))]
mod child {
    use std::io::{Read as _, Write as _};

    use litestd::{
        io::{self as lite, Read as _, Write as _},
        process::{Command, Stdio},
    };

    /// The environment variable that turns this binary into a helper.
    const MODE: &str = "LITESTD_PIPE_CHILD";

    /// The helper side: copies stdin to stdout, and its length to stderr,
    /// then exits. A no-op unless `MODE` is set.
    #[test]
    fn helper() {
        if std::env::var_os(MODE).is_none() {
            return;
        }
        let mut input = Vec::new();
        std::io::stdin().read_to_end(&mut input).unwrap();
        std::io::stdout().write_all(&input).unwrap();
        std::io::stdout().flush().unwrap();
        eprint!("{}", input.len());
        std::process::exit(0);
    }

    /// The arguments that make this binary run only [`helper`].
    const HARNESS: [&str; 5] = [
        "--exact",
        "child::helper",
        "--nocapture",
        "--test-threads=1",
        "-q",
    ];

    fn exe() -> String {
        std::env::current_exe()
            .unwrap()
            .into_os_string()
            .into_string()
            .unwrap()
    }

    /// A child reads a pipe end as stdin and writes another as stdout, then
    /// the same through std, with the same result.
    #[test]
    fn pipe_ends_become_a_child_s_streams() {
        let (child_in, mut to_child) = lite::pipe().unwrap();
        let (mut from_child, child_out) = lite::pipe().unwrap();
        let mut cmd = Command::new(exe());
        cmd.args(HARNESS).env(MODE, "1").stderr(Stdio::null());
        cmd.stdin(child_in).stdout(child_out);
        let mut child = cmd.spawn().unwrap();
        // The command owns the child's ends, which would keep the pipes open.
        drop(cmd);
        to_child.write_all(b"through pipes").unwrap();
        drop(to_child);
        let mut lite_out = Vec::new();
        from_child.read_to_end(&mut lite_out).unwrap();
        assert!(child.wait().unwrap().success());

        let (child_in, mut to_child) = std::io::pipe().unwrap();
        let (mut from_child, child_out) = std::io::pipe().unwrap();
        let mut cmd = std::process::Command::new(exe());
        cmd.args(HARNESS).env(MODE, "1");
        cmd.stderr(std::process::Stdio::null());
        let mut child = cmd.stdin(child_in).stdout(child_out).spawn().unwrap();
        drop(cmd);
        to_child.write_all(b"through pipes").unwrap();
        drop(to_child);
        let mut std_out = Vec::new();
        from_child.read_to_end(&mut std_out).unwrap();
        assert!(child.wait().unwrap().success());
        assert!(lite_out.ends_with(b"through pipes"), "{lite_out:?}");
        assert_eq!(lite_out, std_out);
    }

    /// A child spawned while this process holds a pipe does not inherit its
    /// ends: the reader sees EOF while the child still runs.
    #[test]
    fn children_do_not_inherit_pipe_ends() {
        let (mut reader, writer) = lite::pipe().unwrap();
        let mut cmd = Command::new(exe());
        cmd.args(HARNESS).env(MODE, "1");
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().unwrap();
        drop(writer);
        let mut rest = Vec::new();
        assert_eq!(reader.read_to_end(&mut rest).unwrap(), 0);
        assert!(child.try_wait().unwrap().is_none(), "still reading stdin");
        drop(child.stdin.take());
        assert!(child.wait().unwrap().success());
    }

    /// A writer as stderr, and a clone of it as stdout, as in std's example.
    #[test]
    fn a_writer_and_its_clone_take_two_streams() {
        let (mut reader, writer) = lite::pipe().unwrap();
        let mut cmd = Command::new(exe());
        cmd.args(HARNESS).env(MODE, "1").stdin(Stdio::null());
        cmd.stdout(writer.try_clone().unwrap()).stderr(writer);
        let mut child = cmd.spawn().unwrap();
        drop(cmd);
        let mut out = String::new();
        reader.read_to_string(&mut out).unwrap();
        assert!(child.wait().unwrap().success());
        assert!(out.ends_with('0'), "{out}");
    }
}
