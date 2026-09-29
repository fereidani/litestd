//! `litestd::os::unix::net` against `std::os::unix::net`: addresses,
//! streams, listeners and datagram sockets of either library talking to
//! each other, pathname, unnamed and abstract addresses, errors, `Debug`
//! output and the descriptor traits.
//!
//! Miri runs the address tests, which make no system calls.

#![cfg(all(unix, feature = "net", feature = "path"))]
#![allow(clippy::unwrap_used, reason = "a failure fails the test")]

mod net_support;

use core::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use std::{
    io::{Read as _, Write as _},
    os::unix::ffi::OsStrExt as _,
    path::{Path, PathBuf},
};

use litestd::{
    io::{Read as LiteRead, Write as LiteWrite},
    net::Shutdown,
    os::unix::net::{SocketAddr, UnixDatagram, UnixListener, UnixStream},
};
use net_support::{
    IO_LIMIT, TIMEOUTS, same_error, same_result, same_timeout, timed,
};

/// A directory of its own for one test, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        let name = format!("litestd-unix-{}-{n}", std::process::id());
        let path = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Converts a std path for litestd, which has its own `Path`.
fn lite_path(path: &Path) -> &litestd::path::Path {
    use litestd::os::unix::ffi::OsStrExt;
    litestd::path::Path::new(litestd::ffi::OsStr::from_bytes(
        path.as_os_str().as_bytes(),
    ))
}

/// The bytes of a litestd path.
fn lite_bytes(path: &litestd::path::Path) -> &[u8] {
    use litestd::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes()
}

/// Builds an address with both libraries from the same path bytes and
/// asserts that they agree on everything observable.
#[track_caller]
fn same_addr(path: &[u8]) {
    use litestd::os::unix::ffi::OsStrExt as _;
    let lite = SocketAddr::from_pathname(litestd::ffi::OsStr::from_bytes(path));
    let std = std::os::unix::net::SocketAddr::from_pathname(
        std::ffi::OsStr::from_bytes(path),
    );
    match (lite, std) {
        (Ok(lite), Ok(std)) => {
            assert_eq!(lite.is_unnamed(), std.is_unnamed());
            assert_eq!(
                lite.as_pathname()
                    .map(|p| p.as_os_str().as_bytes().to_vec()),
                std.as_pathname().map(|p| p.as_os_str().as_bytes().to_vec()),
            );
            assert_eq!(format!("{lite:?}"), format!("{std:?}"));
            let clone = lite.clone();
            assert_eq!(format!("{clone:?}"), format!("{std:?}"));
            assert_eq!(clone.is_unnamed(), lite.is_unnamed());
        }
        (Err(lite), Err(std)) => same_error(&lite, &std),
        (lite, std) => {
            assert_eq!(
                format!("{lite:?}"),
                format!("{std:?}"),
                "litestd (left), std (right)"
            );
        }
    }
}

#[test]
fn pathname_addresses_match_std() {
    same_addr(b"/tmp/sock");
    same_addr(b"relative/sock");
    same_addr(b"");
    same_addr(b"with \"quotes\" and \\ and \n");
    same_addr(b"\xff\xfe not utf-8");
    same_addr("caf\u{e9} \u{1f980}".as_bytes());
    // The limits of macOS's 104-byte and Linux's 108-byte paths.
    same_addr(&[b'a'; 103]);
    same_addr(&[b'a'; 104]);
    same_addr(&[b'a'; 107]);
    same_addr(&[b'a'; 108]);
    same_addr(&[b'a'; 300]);
    same_addr(b"nul\0inside");
    same_addr(b"\0leading nul");
}

#[test]
fn addresses_have_std_s_traits() {
    use core::panic::{RefUnwindSafe, UnwindSafe};
    const fn check<T: Send + Sync + Unpin + UnwindSafe + RefUnwindSafe>() {}
    const fn clone<T: Clone>() {}
    check::<SocketAddr>();
    check::<UnixStream>();
    check::<UnixListener>();
    check::<UnixDatagram>();
    check::<litestd::os::unix::net::Incoming<'static>>();
    clone::<SocketAddr>();
}

fn limit_lite(stream: &UnixStream) {
    stream.set_read_timeout(Some(IO_LIMIT)).unwrap();
    stream.set_write_timeout(Some(IO_LIMIT)).unwrap();
}

#[cfg(not(miri))]
#[test]
fn litestd_listener_serves_std_client() {
    timed(|| {
        let dir = TempDir::new();
        let path = dir.join("sock");
        let listener = UnixListener::bind(lite_path(&path)).unwrap();
        let local = listener.local_addr().unwrap();
        assert_eq!(
            lite_bytes(local.as_pathname().unwrap()),
            path.as_os_str().as_bytes()
        );
        let client = std::thread::spawn(move || {
            let mut stream =
                std::os::unix::net::UnixStream::connect(&path).unwrap();
            stream.set_read_timeout(Some(IO_LIMIT)).unwrap();
            stream.write_all(b"hello").unwrap();
            stream.shutdown(std::net::Shutdown::Write).unwrap();
            let mut reply = Vec::new();
            stream.read_to_end(&mut reply).unwrap();
            reply
        });
        let (mut stream, peer) = listener.accept().unwrap();
        limit_lite(&stream);
        assert!(peer.is_unnamed());
        assert_eq!(format!("{peer:?}"), "(unnamed)");
        let mut request = Vec::new();
        stream.read_to_end(&mut request).unwrap();
        assert_eq!(request, b"hello");
        stream.write_all(b"world").unwrap();
        drop(stream);
        assert_eq!(client.join().unwrap(), b"world");
    });
}

#[cfg(not(miri))]
#[test]
fn litestd_client_talks_to_std_listener() {
    timed(|| {
        let dir = TempDir::new();
        let path = dir.join("sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let mut client = UnixStream::connect(lite_path(&path)).unwrap();
        limit_lite(&client);
        let (mut server, _) = listener.accept().unwrap();
        server.set_read_timeout(Some(IO_LIMIT)).unwrap();
        let peer = client.peer_addr().unwrap();
        assert_eq!(
            lite_bytes(peer.as_pathname().unwrap()),
            path.as_os_str().as_bytes()
        );
        assert!(client.local_addr().unwrap().is_unnamed());
        client.write_all(b"ping").unwrap();
        let bufs = [
            litestd::io::IoSlice::new(b" and "),
            litestd::io::IoSlice::new(b"more"),
        ];
        assert_eq!(client.write_vectored(&bufs).unwrap(), 9);
        let mut buf = [0; 13];
        server.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"ping and more");
        server.write_all(b"pong").unwrap();
        let mut reply = [0; 4];
        (&client).read_exact(&mut reply).unwrap();
        assert_eq!(&reply, b"pong");
        // Connecting through an address works as through a path.
        let addr = SocketAddr::from_pathname(lite_path(&path)).unwrap();
        let again = UnixStream::connect_addr(&addr).unwrap();
        drop(listener.accept().unwrap());
        drop(again);
    });
}

#[cfg(not(miri))]
#[test]
fn stream_pairs_behave_like_std() {
    timed(|| {
        let (a, b) = UnixStream::pair().unwrap();
        limit_lite(&a);
        limit_lite(&b);
        (&a).write_all(b"across").unwrap();
        let mut buf = [0; 6];
        (&b).read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"across");
        assert!(a.local_addr().unwrap().is_unnamed());
        assert!(a.peer_addr().unwrap().is_unnamed());
        assert!(a.take_error().unwrap().is_none());

        let mut iov = [0u8; 3];
        (&a).write_all(b"xyz").unwrap();
        let mut bufs = [litestd::io::IoSliceMut::new(&mut iov)];
        assert_eq!((&b).read_vectored(&mut bufs).unwrap(), 3);
        assert_eq!(&iov, b"xyz");

        a.shutdown(Shutdown::Write).unwrap();
        assert_eq!((&b).read(&mut buf).unwrap(), 0);
        let err = (&a).write(b"x").unwrap_err();
        same_error(&err, &std::io::Error::from_raw_os_error(libc::EPIPE));
        // The peer's shutdown closed `b` for reading, so macOS fails to shut
        // it down again with `ENOTCONN` where Linux succeeds; a std pair in
        // the same state does the same.
        let (sa, sb) = std::os::unix::net::UnixStream::pair().unwrap();
        sa.shutdown(std::net::Shutdown::Write).unwrap();
        assert_eq!((&sb).read(&mut buf).unwrap(), 0);
        assert!((&sa).write(b"x").is_err());
        same_result(
            b.shutdown(Shutdown::Both),
            sb.shutdown(std::net::Shutdown::Both),
        );

        let (c, d) = UnixStream::pair().unwrap();
        let clone = c.try_clone().unwrap();
        drop(c);
        (&clone).write_all(b"k").unwrap();
        let mut one = [0; 1];
        (&d).read_exact(&mut one).unwrap();
        assert_eq!(&one, b"k");
        clone.flush_both();
    });
}

/// Flushes a stream through both `Write` implementations, which do nothing.
trait FlushBoth {
    fn flush_both(self);
}

impl FlushBoth for UnixStream {
    fn flush_both(mut self) {
        LiteWrite::flush(&mut self).unwrap();
        LiteWrite::flush(&mut &self).unwrap();
    }
}

#[cfg(not(miri))]
#[test]
fn stream_timeouts_and_nonblocking_mode_match_std() {
    timed(|| {
        let (lite, _lite_peer) = UnixStream::pair().unwrap();
        let (std, _std_peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let own = [
            Some(Duration::from_millis(40)),
            Some(Duration::new(7, 999_999_999)),
        ];
        for dur in own.into_iter().chain(TIMEOUTS) {
            same_result(lite.set_read_timeout(dur), std.set_read_timeout(dur));
            same_result(
                lite.set_write_timeout(dur),
                std.set_write_timeout(dur),
            );
            same_timeout(lite.read_timeout(), std.read_timeout());
            same_timeout(lite.write_timeout(), std.write_timeout());
        }
        let zero = Some(Duration::ZERO);
        same_result(lite.set_read_timeout(zero), std.set_read_timeout(zero));
        same_result(lite.set_write_timeout(zero), std.set_write_timeout(zero));

        let short = Some(Duration::from_millis(40));
        lite.set_read_timeout(short).unwrap();
        std.set_read_timeout(short).unwrap();
        let start = std::time::Instant::now();
        let err = (&lite).read(&mut [0; 4]).unwrap_err();
        assert!(start.elapsed() >= Duration::from_millis(35));
        same_error(&err, &(&std).read(&mut [0; 4]).unwrap_err());

        lite.set_nonblocking(true).unwrap();
        std.set_nonblocking(true).unwrap();
        same_error(
            &(&lite).read(&mut [0; 4]).unwrap_err(),
            &(&std).read(&mut [0; 4]).unwrap_err(),
        );
    });
}

#[cfg(not(miri))]
#[test]
fn errors_match_std() {
    timed(|| {
        let dir = TempDir::new();
        let missing = dir.join("missing");
        same_error(
            &UnixStream::connect(lite_path(&missing)).unwrap_err(),
            &std::os::unix::net::UnixStream::connect(&missing).unwrap_err(),
        );
        let taken = dir.join("taken");
        let _listener = std::os::unix::net::UnixListener::bind(&taken).unwrap();
        same_error(
            &UnixListener::bind(lite_path(&taken)).unwrap_err(),
            &std::os::unix::net::UnixListener::bind(&taken).unwrap_err(),
        );
        same_error(
            &UnixDatagram::bind(lite_path(&taken)).unwrap_err(),
            &std::os::unix::net::UnixDatagram::bind(&taken).unwrap_err(),
        );
        let long = dir.join(&"x".repeat(200));
        same_error(
            &UnixStream::connect(lite_path(&long)).unwrap_err(),
            &std::os::unix::net::UnixStream::connect(&long).unwrap_err(),
        );
        same_error(
            &UnixListener::bind("a\0b").unwrap_err(),
            &std::os::unix::net::UnixListener::bind("a\0b").unwrap_err(),
        );
        // A stream socket connecting to a datagram socket.
        let dgram = dir.join("dgram");
        let _d = std::os::unix::net::UnixDatagram::bind(&dgram).unwrap();
        same_error(
            &UnixStream::connect(lite_path(&dgram)).unwrap_err(),
            &std::os::unix::net::UnixStream::connect(&dgram).unwrap_err(),
        );
        // A datagram socket that is not connected.
        let lite = UnixDatagram::unbound().unwrap();
        let std = std::os::unix::net::UnixDatagram::unbound().unwrap();
        same_result(lite.send(b"x"), std.send(b"x"));
        same_result(
            lite.peer_addr().map(|a| format!("{a:?}")),
            std.peer_addr().map(|a| format!("{a:?}")),
        );
        same_result(
            lite.shutdown(Shutdown::Read),
            std.shutdown(std::net::Shutdown::Read),
        );
        same_result(
            lite.send_to(b"x", lite_path(&missing)),
            std.send_to(b"x", &missing),
        );
    });
}

#[cfg(not(miri))]
#[test]
fn datagrams_flow_both_ways() {
    timed(|| {
        let dir = TempDir::new();
        let lite_path_buf = dir.join("lite");
        let std_path = dir.join("std");
        let lite = UnixDatagram::bind(lite_path(&lite_path_buf)).unwrap();
        lite.set_read_timeout(Some(IO_LIMIT)).unwrap();
        let std = std::os::unix::net::UnixDatagram::bind(&std_path).unwrap();
        std.set_read_timeout(Some(IO_LIMIT)).unwrap();

        assert_eq!(lite.send_to(b"to std", lite_path(&std_path)).unwrap(), 6);
        let mut buf = [0; 16];
        let (n, from) = std.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"to std");
        assert_eq!(from.as_pathname(), Some(lite_path_buf.as_path()));
        assert_eq!(
            lite_bytes(lite.local_addr().unwrap().as_pathname().unwrap()),
            lite_path_buf.as_os_str().as_bytes()
        );

        std.send_to(b"to lite", &lite_path_buf).unwrap();
        let (n, from) = lite.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"to lite");
        assert_eq!(
            lite_bytes(from.as_pathname().unwrap()),
            std_path.as_os_str().as_bytes()
        );

        // From an unbound socket the sender is unnamed.
        let anon = std::os::unix::net::UnixDatagram::unbound().unwrap();
        anon.send_to(b"", &lite_path_buf).unwrap();
        let (n, from) = lite.recv_from(&mut buf).unwrap();
        assert_eq!(n, 0);
        assert!(from.is_unnamed());
        assert_eq!(format!("{from:?}"), "(unnamed)");

        // Connected: `send` and `recv`.
        let addr = lite.local_addr().unwrap();
        let sender = UnixDatagram::unbound().unwrap();
        sender.connect_addr(&addr).unwrap();
        assert_eq!(
            format!("{:?}", sender.peer_addr().unwrap()),
            format!("{:?}", lite.local_addr().unwrap())
        );
        sender.send(b"connected").unwrap();
        assert_eq!(lite.recv(&mut buf).unwrap(), 9);
        sender.connect(lite_path(&std_path)).unwrap();
        // Sending to another address from a connected socket works on Linux
        // and fails with `EISCONN` on macOS, for std as for litestd.
        let std_sender = std::os::unix::net::UnixDatagram::unbound().unwrap();
        std_sender.connect(&std_path).unwrap();
        let to_lite =
            std::os::unix::net::SocketAddr::from_pathname(&lite_path_buf)
                .unwrap();
        let sent = sender.send_to_addr(b"by addr", &addr);
        let delivered = sent.is_ok();
        same_result(sent, std_sender.send_to_addr(b"by addr", &to_lite));
        if delivered {
            assert_eq!(lite.recv(&mut buf).unwrap(), 7);
            assert_eq!(lite.recv(&mut buf).unwrap(), 7);
        }
        let bound_again = UnixDatagram::bind_addr(
            &SocketAddr::from_pathname(lite_path(&dir.join("third"))).unwrap(),
        )
        .unwrap();
        assert!(!bound_again.local_addr().unwrap().is_unnamed());
    });
}

#[cfg(not(miri))]
#[test]
fn datagram_pairs_and_options() {
    timed(|| {
        let (a, b) = UnixDatagram::pair().unwrap();
        b.set_read_timeout(Some(IO_LIMIT)).unwrap();
        a.send(b"one").unwrap();
        a.send(b"two!").unwrap();
        let mut buf = [0; 2];
        assert_eq!(b.recv(&mut buf).unwrap(), 2);
        let mut buf = [0; 8];
        assert_eq!(b.recv(&mut buf).unwrap(), 4);
        let clone = a.try_clone().unwrap();
        clone.send(b"c").unwrap();
        assert_eq!(b.recv_from(&mut buf).unwrap().0, 1);
        assert!(a.take_error().unwrap().is_none());

        let (std_a, _std_b) = std::os::unix::net::UnixDatagram::pair().unwrap();
        let own = [Some(Duration::from_millis(3))];
        for dur in own.into_iter().chain(TIMEOUTS) {
            same_result(a.set_read_timeout(dur), std_a.set_read_timeout(dur));
            same_result(a.set_write_timeout(dur), std_a.set_write_timeout(dur));
            same_timeout(a.read_timeout(), std_a.read_timeout());
            same_timeout(a.write_timeout(), std_a.write_timeout());
        }
        same_result(
            a.set_read_timeout(Some(Duration::ZERO)),
            std_a.set_read_timeout(Some(Duration::ZERO)),
        );
        a.set_nonblocking(true).unwrap();
        std_a.set_nonblocking(true).unwrap();
        same_error(
            &a.recv(&mut buf).unwrap_err(),
            &std_a.recv(&mut buf).unwrap_err(),
        );
        a.shutdown(Shutdown::Both).unwrap();
        same_result(a.send(b"x"), {
            std_a.shutdown(std::net::Shutdown::Both).unwrap();
            std_a.send(b"x")
        });
    });
}

/// An abstract name from a std socket, seen through litestd over the same
/// descriptor, and litestd connecting to it with the address it reported.
#[cfg(all(target_os = "linux", not(miri)))]
#[test]
fn abstract_addresses_interoperate() {
    use std::os::{fd::AsRawFd, linux::net::SocketAddrExt as _};
    timed(|| {
        let name =
            format!("litestd-abstract-{}\0\"\\\n\u{e9}", std::process::id());
        let mut name = name.into_bytes();
        name.push(0xff);
        let std_addr =
            std::os::unix::net::SocketAddr::from_abstract_name(&name).unwrap();
        let std_listener =
            std::os::unix::net::UnixListener::bind_addr(&std_addr).unwrap();
        // SAFETY: the litestd listener borrows the descriptor and is never
        // dropped, so std closes it once.
        let lite_listener = core::mem::ManuallyDrop::new(unsafe {
            <UnixListener as litestd::os::fd::FromRawFd>::from_raw_fd(
                std_listener.as_raw_fd(),
            )
        });
        let lite_addr = lite_listener.local_addr().unwrap();
        let std_local = std_listener.local_addr().unwrap();
        assert!(!lite_addr.is_unnamed());
        assert_eq!(lite_addr.as_pathname(), None);
        assert_eq!(format!("{lite_addr:?}"), format!("{std_local:?}"));
        assert_eq!(
            format!("{:?}", *lite_listener),
            format!("{std_listener:?}")
        );

        let lite_client = UnixStream::connect_addr(&lite_addr).unwrap();
        let (std_server, _) = std_listener.accept().unwrap();
        let peer = lite_client.peer_addr().unwrap();
        assert_eq!(format!("{peer:?}"), format!("{std_local:?}"));
        // SAFETY: as above, for the accepted stream.
        let lite_server = core::mem::ManuallyDrop::new(unsafe {
            <UnixStream as litestd::os::fd::FromRawFd>::from_raw_fd(
                std_server.as_raw_fd(),
            )
        });
        assert_eq!(format!("{:?}", *lite_server), format!("{std_server:?}"));
    });
}

/// Binding to an empty path makes Linux pick an abstract name.
#[cfg(all(target_os = "linux", not(miri)))]
#[test]
fn autobind_picks_an_abstract_name() {
    timed(|| {
        let lite = UnixDatagram::bind("").unwrap();
        let std = std::os::unix::net::UnixDatagram::bind("").unwrap();
        let (lite_addr, std_addr) =
            (lite.local_addr().unwrap(), std.local_addr().unwrap());
        assert!(!lite_addr.is_unnamed());
        assert!(
            format!("{lite_addr:?}").ends_with(" (abstract)"),
            "{lite_addr:?}"
        );
        assert_eq!(lite_addr.is_unnamed(), std_addr.is_unnamed());
        // A datagram to the address it reported arrives.
        std.send_to_addr(b"auto", &{
            use std::os::linux::net::SocketAddrExt as _;
            let debug = format!("{lite_addr:?}");
            let name = debug.trim_end_matches(" (abstract)").trim_matches('"');
            std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())
                .unwrap()
        })
        .unwrap();
        lite.set_read_timeout(Some(IO_LIMIT)).unwrap();
        let mut buf = [0; 4];
        let (n, from) = lite.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"auto");
        assert!(!from.is_unnamed());
    });
}

/// litestd's `FromRawFd`, which a scope with std's cannot name as a
/// method.
///
/// # Safety
///
/// As for `FromRawFd::from_raw_fd`.
unsafe fn lite_from_raw<T: litestd::os::fd::FromRawFd>(fd: i32) -> T {
    // SAFETY: the caller upholds the contract.
    unsafe { T::from_raw_fd(fd) }
}

#[cfg(not(miri))]
#[test]
fn debug_output_matches_std() {
    use std::os::fd::AsRawFd;
    timed(|| {
        let dir = TempDir::new();
        let path = dir.join("debug \"sock\"");
        let std_listener =
            std::os::unix::net::UnixListener::bind(&path).unwrap();
        let std_client =
            std::os::unix::net::UnixStream::connect(&path).unwrap();
        let std_dgram =
            std::os::unix::net::UnixDatagram::bind(dir.join("d")).unwrap();
        std_dgram.connect(dir.join("d")).unwrap();
        let (std_pair, _other) =
            std::os::unix::net::UnixStream::pair().unwrap();
        // SAFETY: the litestd values borrow the std values' descriptors and
        // are never dropped, so std closes each once.
        let (listener, client, dgram, pair) = unsafe {
            (
                core::mem::ManuallyDrop::new(lite_from_raw::<UnixListener>(
                    std_listener.as_raw_fd(),
                )),
                core::mem::ManuallyDrop::new(lite_from_raw::<UnixStream>(
                    std_client.as_raw_fd(),
                )),
                core::mem::ManuallyDrop::new(lite_from_raw::<UnixDatagram>(
                    std_dgram.as_raw_fd(),
                )),
                core::mem::ManuallyDrop::new(lite_from_raw::<UnixStream>(
                    std_pair.as_raw_fd(),
                )),
            )
        };
        assert_eq!(format!("{:?}", *listener), format!("{std_listener:?}"));
        assert_eq!(format!("{:?}", *client), format!("{std_client:?}"));
        assert_eq!(format!("{:?}", *dgram), format!("{std_dgram:?}"));
        assert_eq!(format!("{:?}", *pair), format!("{std_pair:?}"));
        assert_eq!(format!("{:#?}", *client), format!("{std_client:#?}"));
    });
}

#[cfg(not(miri))]
#[test]
fn incoming_iterates_connections() {
    timed(|| {
        let dir = TempDir::new();
        let path = dir.join("sock");
        let listener = UnixListener::bind(lite_path(&path)).unwrap();
        let clients: Vec<_> = (0..3)
            .map(|_| std::os::unix::net::UnixStream::connect(&path).unwrap())
            .collect();
        let incoming = listener.incoming();
        assert_eq!(incoming.size_hint(), (usize::MAX, None));
        assert!(format!("{incoming:?}").starts_with(
            "Incoming { listener: UnixListener { fd: FileDesc(OwnedFd { fd: "
        ));
        let accepted: Vec<_> = incoming.take(2).map(Result::unwrap).collect();
        assert_eq!(accepted.len(), 2);
        let third = (&listener).into_iter().next().unwrap().unwrap();
        drop((accepted, third, clients));
        let clone = listener.try_clone().unwrap();
        assert!(clone.take_error().unwrap().is_none());
        clone.set_nonblocking(true).unwrap();
        let std_listener =
            std::os::unix::net::UnixListener::bind(dir.join("s2")).unwrap();
        std_listener.set_nonblocking(true).unwrap();
        same_error(
            &listener.accept().unwrap_err(),
            &std_listener.accept().unwrap_err(),
        );
    });
}

#[cfg(not(miri))]
#[test]
fn descriptor_traits_convert() {
    use litestd::os::fd::{AsFd, AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
    timed(|| {
        let (a, b) = UnixStream::pair().unwrap();
        let raw = a.as_raw_fd();
        assert_eq!(a.as_fd().as_raw_fd(), raw);
        let a = UnixStream::from(OwnedFd::from(a));
        // SAFETY: the descriptor passes from one owner to the next.
        let a = unsafe { UnixStream::from_raw_fd(a.into_raw_fd()) };
        assert_eq!(a.as_raw_fd(), raw);
        (&a).write_all(b"ok").unwrap();
        let mut buf = [0; 2];
        (&b).read_exact(&mut buf).unwrap();

        let (d, _e) = UnixDatagram::pair().unwrap();
        let raw = d.as_raw_fd();
        let d = UnixDatagram::from(OwnedFd::from(d));
        // SAFETY: as above.
        let d = unsafe { UnixDatagram::from_raw_fd(d.into_raw_fd()) };
        assert_eq!((d.as_raw_fd(), d.as_fd().as_raw_fd()), (raw, raw));

        let dir = TempDir::new();
        let listener = UnixListener::bind(lite_path(&dir.join("l"))).unwrap();
        let raw = listener.as_raw_fd();
        let listener = UnixListener::from(OwnedFd::from(listener));
        // SAFETY: as above.
        let listener =
            unsafe { UnixListener::from_raw_fd(listener.into_raw_fd()) };
        assert_eq!(
            (listener.as_raw_fd(), listener.as_fd().as_raw_fd()),
            (raw, raw)
        );
    });
}

/// In a process where `SIGPIPE` has its default action, as in a litestd
/// program, writing to a closed stream must fail rather than raise the
/// signal, which would kill the process.
#[cfg(not(miri))]
#[test]
fn writes_do_not_raise_sigpipe() {
    if net_support::is_child() {
        net_support::default_sigpipe();
        let (a, b) = UnixStream::pair().unwrap();
        drop(b);
        let err = (&a).write(b"x").unwrap_err();
        assert_eq!(err.kind(), litestd::io::ErrorKind::BrokenPipe);
        let bufs = [litestd::io::IoSlice::new(b"y")];
        assert!((&a).write_vectored(&bufs).is_err());
        let (c, d) = UnixDatagram::pair().unwrap();
        c.shutdown(Shutdown::Write).unwrap();
        assert!(c.send(b"z").is_err());
        drop(d);
        println!("survived");
        return;
    }
    let out = net_support::run_child("writes_do_not_raise_sigpipe");
    assert!(out.status.success(), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("survived"),
        "{out:?}"
    );
}
